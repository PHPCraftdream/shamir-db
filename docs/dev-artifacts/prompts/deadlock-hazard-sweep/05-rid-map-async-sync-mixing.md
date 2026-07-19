# Deadlock hazard — `HnswAdapter::rid_map` mixes scc `_async`/`_sync` lock acquisition (found during the H3 fix)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

## Background — this is a follow-up to the just-landed H3 fix

`git show dcfaf825` (the H3 fix: `deleted`/`vectors_u8`/`vectors`/
`rid_to_internal`/`compaction_deleted_rids` in
`crates/shamir-index/src/vector/hnsw_adapter.rs` +
`vector_backend.rs`) — the implementing agent, while sweeping those 5
maps, independently found that **`rid_map`** (a SIXTH map in the same
file, NOT one of the 5 originally named in that task's scope) has the
IDENTICAL structural hazard, and correctly left it untouched per the
surgical-changes rule, flagging it as a recommended follow-up. This task
IS that follow-up.

Read `git show 7a4abf62` (`#589`) and `git show dcfaf825` (H3) first for
the established mechanism and doc-comment style — this task mirrors H3
almost exactly, just for one additional map in the same file.

## The map

`rid_map: scc::HashMap<usize, RecordId, THasher>` in
`crates/shamir-index/src/vector/hnsw_adapter.rs` — maps internal ids back
to `RecordId`s (the reverse of `rid_to_internal`, which H3 already fixed).

**Confirmed via direct grep (verify these yourself before editing — line
numbers may have shifted since this brief was written):**
- `iter_sync` — ONE site, in `for_each_rid_map` (snapshot serialisation) —
  the SYNCHRONOUS accessor that makes the other sites' `_async` calls a
  hazard.
- `insert_async` — SIX sites, across `upsert`, `upsert_batch`,
  `backfill_if_absent`, and related internal-id claim paths (grep
  `self.rid_map.insert_async` to enumerate them precisely).
- `read_async` — SIX sites, across the search paths (`search_quantized_graph`,
  `search_cofilter_quantized`, `search_prefilter`, `search_quantized_bruteforce`,
  and others — grep `self.rid_map.read_async` to enumerate them precisely).

No `entry_async`/`remove_async`/`contains_async` sites exist on this map
(confirmed by grep) — only the two shapes above (`insert_async`,
`read_async`) plus the one `iter_sync`.

## The hazard (identical mechanism to H1-H3 — same summary, one map)

`scc::HashMap`'s `_async` lock-wait is a lock-HANDOFF: on release, the
bucket lock is granted directly to a suspended waiter TASK, which then
holds it while sitting in tokio's run queue until re-polled. The `_sync`
`iter_sync` accessor instead PARKS the calling OS thread while a bucket is
locked. If a worker running `for_each_rid_map`'s `iter_sync` scan parks on
a bucket that saa just handed off to a suspended `insert_async`/`read_async`
task (from a concurrent upsert or search), and enough workers pile up the
same way, the lock-owning task is never polled again → whole-runtime
deadlock. `for_each_rid_map` runs during snapshot serialisation (an
ordinary async fn on the runtime, not `spawn_blocking`), concurrent with
ordinary upsert/search traffic — a live hazard, not theoretical.

## The fix — mechanical, identical shape to H3

Convert all `insert_async` and `read_async` call sites on `rid_map` to
`insert_sync`/`read_sync`. None of the closures involved suspend (they are
plain inserts and dereferences), so this is mechanical. Add a "DEADLOCK
FIX (same class as #589, commit `7a4abf62`; H3 commit `dcfaf825`)" doc
comment at each converted site, matching the established style from the
H3 commit exactly (name `for_each_rid_map`'s `iter_sync` as the specific
synchronous accessor that makes each conversion necessary).

## Tests

Mirror the H3 fix's `crates/shamir-index/src/vector/tests/
deadlock_regression_tests.rs` pattern — add a test to that SAME file (it
already exists from the H3 fix; extend it rather than creating a new
module unless you judge a fresh file cleaner) exercising: a constrained
runtime (`worker_threads = 1` or `2`), concurrent upsert/search traffic
(which hits `rid_map.insert_async`/`read_async`) racing a snapshot-
serialisation call (which hits `for_each_rid_map`'s `iter_sync`) — wrapped
in a NAMED bounded `tokio::time::timeout` (mirror the existing H3 tests'
60s pattern and failure-message style) so a future regression fails fast
and identifiably. Document (matching the established convention) that this
is a race-window regression guard, not a guaranteed deterministic
reproducer.

Existing vector-index tests (search, snapshot serialisation, compaction)
must continue to pass unchanged — this fix only changes the lock-
acquisition mechanism, not any observable search result or snapshot
content.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-index --full` green, including the new
  test.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-index`, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm via grep: zero `insert_async`/`read_async`/
  `entry_async`/`contains_async`/`remove_async` calls remain on `rid_map`
  anywhere in the crate.
- Confirm no existing vector-index test's observable behavior (search
  results, snapshot content) changed — only the lock mechanism.

## Out of scope

- Do NOT touch the 5 maps H3 already fixed (`deleted`, `vectors_u8`,
  `vectors`, `rid_to_internal`, `compaction_deleted_rids`) — confirm they
  remain untouched.
- Do NOT touch `MvccStore::cells` (#589), `RepoTxGate::active_snapshots`/
  `MvccStore::locks` (H1+H2), `per_table_mvcc`/`token_names` (H4+H5), or
  `layered_interner.rs` (H6) — all already fixed in prior commits of this
  same sweep.
- Do NOT change any vector-index ALGORITHM (search ranking, snapshot
  format, quantization math) — this task is entirely about lock-
  acquisition mechanism on `rid_map`, nothing else.
- Do NOT raise any test timeout to paper over a hang — if you observe a
  real hang during testing, root-cause it rather than loosening a timeout.
