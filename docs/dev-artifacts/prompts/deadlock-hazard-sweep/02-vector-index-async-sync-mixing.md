# Deadlock hazard — vector index (HNSW) mixes scc `_async`/`_sync` lock acquisition across 5 shared maps

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

## Background — the proven mechanism (read `git show 7a4abf62` and the
## already-fixed sibling task's commit first — search `git log --oneline
## --grep "H1+H2"` for the immediately-prior commit in this same sweep)

`scc::HashMap`'s async lock-wait path (`entry_async(...).await`,
`insert_async`, `remove_async`, `read_async`, etc.) is a **lock-HANDOFF**:
on release, the exclusive (or shared) bucket lock is granted DIRECTLY to a
suspended waiter TASK, which then holds it while merely sitting in tokio's
run queue until it is next polled. Synchronous accessors on the SAME map
(`entry_sync`, `insert_sync`, `remove_sync`, `read_sync`, `contains_sync`,
`iter_sync`, ...) instead **park the calling OS thread** while the bucket is
locked. If every runtime worker thread happens to park on the same bucket
during the handoff window, the lock-owning task can never be polled again —
**whole-runtime deadlock**. This class was proven and fixed for `MvccStore`'s
`cells` map (`#589`, commit `7a4abf62`) and for `RepoTxGate::active_snapshots`
+ `MvccStore::locks` (the immediately-prior commit in this same task series —
read it for the established doc-comment style to mirror). **The fix
convention: every lock-acquiring op on a map that has ANY synchronous
accessor must ITSELF be synchronous — never mix `_async` and `_sync` lock
acquisition on the same scc map.**

A read-only research sweep found the SAME structural violation, replicated
across **five** shared maps inside the vector/HNSW index subsystem —
`crates/shamir-index/src/vector/hnsw_adapter.rs` (four of the five maps) and
`crates/shamir-index/src/vector/vector_backend.rs` (the fifth, cross-file).
The `_sync` accessors run on ordinary runtime WORKER threads (search /
compaction / fit are regular `async fn`s; only the CPU-bound graph-insert
step itself is `spawn_blocking`), so this is a live hazard, not a
theoretical one. **This is the largest/most mechanical fix in this sweep —
read the whole background above, then read `hnsw_adapter.rs` and
`vector_backend.rs` in full (they are large; budget real time for this) to
find every current call site before editing — line numbers below are from
the research pass and may have drifted; grep for the exact call sites
yourself rather than trusting the numbers verbatim.**

## The five maps

1. **`deleted`** (tombstones, keyed by internal id) — `hnsw_adapter.rs`.
   Async: `insert_async` (the `delete()` path), `contains_async` (search
   bruteforce loop — note the file may use BOTH `contains_async` and
   `contains_sync` spellings for conceptually the same probe; unify on
   `contains_sync`). Sync: `contains_sync` (multiple sites — search hot
   paths including `quantized_fastpath_publish`'s upsert fast path),
   `insert_sync` (at least two sites), `iter_sync` (snapshot serialisation /
   `collect_live_vectors`). **AMPLIFIER — read this carefully**: at least
   one `insert_sync` call on `deleted` executes WHILE the caller HOLDS an
   exclusive `rid_to_internal` entry acquired via `entry_async` (the guard
   is dropped only after this call) — a worker parked in `deleted.insert_sync`
   therefore parks WHILE OWNING the `rid_to_internal` bucket, chaining two
   maps together and widening the deadlock window beyond the single-map
   case. Fixing `rid_to_internal`'s `entry_async` to `entry_sync` (map 4
   below) closes this specific chain; still convert `deleted`'s own
   `insert_async`/`contains_async` to sync for map-4-independent safety.

2. **`vectors_u8`** (quantized codes) — `hnsw_adapter.rs`. Async:
   `insert_async`/`remove_async`/`read_async` (many sites across
   upsert/delete/compaction paths). Sync: `iter_sync` (fit snapshot scans,
   AND — critically — `search_quantized_bruteforce`, a **LIVE QUERY PATH**
   synchronously iterating this map while it is concurrently mutated via
   `_async` calls elsewhere), `read_sync`, `insert_sync`, `contains_sync`.

3. **`vectors`** (f32 buffer) — `hnsw_adapter.rs`. Async:
   `insert_async`/`remove_async` (upsert/delete/compaction paths). Sync:
   `iter_sync` (fit snapshot/delta/catch-up scans — these run in ordinary
   async fns on the runtime, not `spawn_blocking`), `read_sync`,
   `remove_sync`.

4. **`rid_to_internal`** — `hnsw_adapter.rs`. Async exclusive: `entry_async`
   (`backfill_if_absent`, `upsert`, `upsert_batch` — the site referenced in
   the map-1 amplifier note above), `read_async`. Sync: `iter_sync`
   (snapshot serialisation, `collect_live_vectors`/compaction),
   `contains_sync`.

5. **`compaction_deleted_rids`** (cross-file) — declared/used across
   `hnsw_adapter.rs` AND `vector_backend.rs`. Async: `insert_async` in
   `vector_backend.rs` (the delete double-write path — every user delete
   DURING a compaction). Sync: `contains_sync` in `hnsw_adapter.rs`
   (backfill check + under-entry-lock re-check), `iter_sync` in
   `vector_backend.rs` (compaction reconcile step).

## Concrete failure scenario (the cleanest single-map variant, `deleted`)

1. During or after a compaction burst, a `delete(rid)` task suspends in
   `deleted.insert_async(internal)` because an upsert's `insert_sync`
   briefly owns the bucket; on release, saa **grants the delete task the
   exclusive bucket lock** while it sits in the run queue, unpolled.
2. Concurrent search tasks (every candidate-filter probe during a search is
   `deleted.contains_sync` — hit on multiple search-path call sites) **park
   their worker threads** on that same bucket.
3. With search concurrency ≥ worker count (an ordinary query burst), all
   workers park → the delete task never polled → whole-runtime deadlock.
   On a `worker_threads=1` runtime, one search racing one delete suffices.

Unlike the `cells`/`active_snapshots` maps fixed in prior tasks, there is no
single naturally-hot key here (internal ids spread across many buckets), so
this needs either a hot-record delete/upsert churn colliding with a search
burst on the SAME shard, a compaction window (`compaction_deleted_rids` IS a
single shared map hammered during double-write, so it behaves more like the
hot-key case), or a small/constrained runtime to reliably expose it in a
test.

## The fix — mechanical, same shape as the prior two commits in this sweep

Make EVERY lock-acquiring op on all FIVE maps synchronous:
`insert_sync`/`remove_sync`/`entry_sync`/`read_sync`/`contains_sync`/
`iter_sync` (the last is already sync everywhere it's used — just confirm
no stray `iter_async` exists on these maps). None of the closures involved
suspend (they are simple inserts/removes/reads/containment checks), so this
is a mechanical `_async`→`_sync` sweep, not a redesign — confirmed safe by
the same reasoning as the prior two fixes in this series. This also erases
the map-1 amplifier chain described above (once `rid_to_internal`'s
`entry_async` sites become `entry_sync`, a worker can no longer park while
holding that bucket across an `.await`).

Add a "DEADLOCK FIX (same class as #589, commit `7a4abf62`)" doc comment at
each converted call site, matching the style established in the immediately
-prior commit of this same sweep (`git log -1 --format=%H` on the commit
just before this task, or search for the exact H1/H2 commit) — name the
specific synchronous accessor(s) on the SAME map that make the conversion
necessary, exactly as that commit's comments do.

## Tests

Read the existing `stress_concurrent_mutations_during_quantized_compaction`
test (find it in the vector index test suite — search for
`quantized_compaction`/`stress_concurrent` under `crates/shamir-index/src/
vector/tests/`) first — it is the report's own suggested skeleton to extend.

1. **Primary regression**: a stress test on a SMALL/constrained runtime
   (`worker_threads = 1` or `2`) with a quantized adapter PAST
   `FIT_THRESHOLD` (find this constant and how existing tests reach that
   state), looping: {concurrent delete + re-upsert of ONE rid} racing
   {concurrent `search_quantized_bruteforce`-sized searches}. This is the
   report's own suggested repro direction for the `deleted`/`vectors_u8`
   maps' hazard.
2. **Compaction variant**: extend the existing
   `stress_concurrent_mutations_during_quantized_compaction` test with
   concurrent DELETES layered into the compaction window (targeting
   `compaction_deleted_rids`'s hazard specifically, since that map is
   genuinely shared/hammered during double-write, unlike the others'
   spread-key nature).
3. Wrap any exercising loop in a bounded `tokio::time::timeout` with a
   NAMED assertion message (mirror `quantized_graph_tests.rs:1630` and the
   style of the prior two tasks' new tests in this series) — a real
   regression must fail fast and identifiably, not hang the whole nextest
   run. Document (in the test module doc comment, matching the established
   style) that this is a race-window regression guard, not a deterministic
   reproducer.
4. Existing vector-index tests (search, compaction, snapshot
   serialisation, quantized fastpath) must continue to pass unchanged —
   this fix only changes HOW locks are acquired, not any observable search
   result, compaction outcome, or snapshot content.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-index --full` green, including all new
  tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-index`, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) EVERY lock-acquiring call site on all 5 maps is
  now synchronous — list them (you found them yourself via grep, so this
  should be a straightforward enumeration) and confirm none was missed,
  (b) the `rid_to_internal`-holding-while-`deleted.insert_sync`-parks
  amplifier chain is closed by the `rid_to_internal` conversion, (c) no
  existing vector-index test's observable behavior (search results,
  compaction correctness, snapshot content) changed — only the internal
  lock-acquisition mechanism.

## Out of scope

- Do NOT touch `MvccStore::cells` (already fixed, `#589`),
  `RepoTxGate::active_snapshots`/`MvccStore::locks` (already fixed, the
  immediately-prior commit in this sweep), `per_table_mvcc`/`token_names`
  (`repo_instance.rs` — a separate, already-tracked follow-up task, H4/H5),
  or `layered_interner.rs` (a separate, already-tracked, lowest-priority
  follow-up task, H6).
- Do NOT change any vector-index ALGORITHM (search ranking, compaction
  logic, quantization math) — this task is entirely about lock-acquisition
  mechanism on the 5 named maps, nothing else.
- Do NOT raise any test timeout to paper over a hang — if you observe a
  real hang during testing, root-cause it (per this session's standing
  "hunt and fix hangs, never tolerate" discipline) rather than loosening a
  timeout.
