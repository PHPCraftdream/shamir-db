Task #536 — redesign the fjall worker-loop batching idea from the reverted
task #524 prototype into a SAFE, narrower form: a write-only dedicated
worker thread, leaving reads on the existing per-op `spawn_blocking` path.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context — READ THIS FIRST, do not re-derive from scratch

`docs/dev-artifacts/design/fjall-worker-loop-524-findings.md` documents the FULL
investigation behind this task: #524 prototyped funneling ALL point-ops
(read AND write) through one dedicated OS worker thread. `@fl` adversarial
review found this discards fjall's genuine multi-threaded read parallelism
(confirmed against `lsm-tree`/fjall source + fjall's own README: reads take
a brief shared lock to clone an immutable snapshot, then proceed lock-free
— many threads reading concurrently is the INTENDED usage). The prototype's
own bench (1MB, entirely memtable-resident) could not expose this because
every "read" was a sub-microsecond skiplist lookup regardless of thread
count. The prototype was reverted, never committed.

**This task is the follow-up redesign that document recommended**: split
routing by operation kind. Writes go through one dedicated worker (fjall
already serializes writes on its own journal mutex internally, so batching
them onto one thread loses NO real parallelism — a clean, safe win).
Reads stay on the existing `spawn_blocking` path (the current,
already-correct behavior — see `crates/shamir-storage/src/storage_fjall.rs`)
UNLESS you find, with real cold-cache measurement, that a sharded reader
pool is clearly worth the added complexity — the task's own instructions
treat "keep reads as-is" as an acceptable, conservative outcome, not a
consolation prize.

## The change

1. Add a single dedicated OS worker thread per `FjallStore` (mirror #524's
   MPSC + oneshot-reply plumbing for the CHANNEL mechanics — that part of
   the reverted prototype was fine, only the "route reads through it too"
   decision was wrong) that owns one clone of the `fjall::Keyspace` handle.
2. Route ONLY the write ops through this worker: `insert`, `set`, `remove`,
   whatever batched write variants exist (`set_many`/`remove_many` if
   present in this file — grep for them). Each of these currently uses its
   own `task::spawn_blocking` call (see `crates/shamir-storage/src/
   storage_fjall.rs` lines ~97-168, ~322+, and any `_many` variants) —
   replace those specific call sites with a submit-to-worker call.
3. Leave `get`/`get_many`/`iter_stream`/`scan_prefix_stream`/anything
   read-only UNCHANGED — still on `spawn_blocking`, still genuinely
   multi-threaded.
4. Preserve the exact same error handling / return shapes each method
   currently has (`DbResult<RecordKey>`, `DbResult<bool>`, etc.) — this is
   purely a dispatch-mechanism change, not a behavior change.

## The TOCTOU claim — VERIFY, do not just assert

The task description (and this file's context) claims the write-worker
"incidentally closes the contains_key-then-insert TOCTOU window from audit
finding 1.2/§B13" (see `set`'s existing doc comment at
`storage_fjall.rs:149-155` — "§B13 (acknowledged TOCTOU)... Concurrent
calls from outside the engine... would race"). Investigate whether routing
ALL writes through one serialized worker thread actually changes anything
here:
- If EVERY write op (not just `set`) submits through the same single
  worker and the worker processes submissions strictly one-at-a-time (no
  reordering, no concurrent execution of two write ops), then yes — two
  concurrent `set(same_key)` calls from different callers now execute
  serially in submission order, closing the TOCTOU for real (whichever
  submitted second sees the first one's committed state at its
  `contains_key` check, deterministically — no longer a genuine race).
- If this is true, update the doc comment at `storage_fjall.rs:149-155` to
  say so (removing the "acknowledged TOCTOU" framing) — but ONLY if you've
  actually confirmed it, not because this brief said it might be true.
  If it's NOT actually closed (e.g. because of some batching/reordering
  detail in your specific implementation), leave the existing honest
  comment alone and say so in your report.

## Bench — MANDATORY requirement, do not reuse #524's insufficient bench

The existing `storage_fjall_pump.rs` bench (confirmed sufficient for task
#523's baseline numbers: insert 46,095 ns/op, get 29,887 ns/op, set_existing
50,443 ns/op, scan_prefix 525,893 ns/op) used a small, likely
memtable-resident dataset. For THIS task, add a bench variant (or a new
bench file) whose dataset is SEVERAL TIMES LARGER than fjall's block/cache
layer — enough to force real cold-cache disk reads and, critically,
concurrent WRITE contention under realistic fan-out (e.g. 16-64 concurrent
writers hammering `insert`/`set` on a table with enough existing data that
writes aren't trivially memtable-only). Measure BOTH:
1. Write throughput/latency before vs. after (the expected win — dispatch
   amortization, one thread instead of N-per-op `spawn_blocking` hops).
2. Read throughput/latency before vs. after (MUST show no regression —
   reads are untouched, but confirm empirically rather than just asserting
   it, since a subtle mistake in wiring could accidentally route a read
   through the write worker).

Report the numbers HONESTLY — if the write-side win is smaller than
expected, or reads show ANY regression (which would indicate a wiring bug,
not an acceptable trade-off), say so plainly. Do not fabricate a signal.
If reads regress at all, STOP and investigate — that would mean something
got mis-routed, not an expected trade-off of this design.

## Scope-down guidance

If, during implementation, you find the write-worker's serialization adds
NET latency to burst-write workloads that matters more than the dispatch
savings (e.g. because fjall's own internal journal mutex already imposes
the same serialization, so adding an EXTRA queueing hop on top is pure
overhead with no compensating benefit) — STOP, document the finding, and
recommend reverting to the current per-op `spawn_blocking` write path
rather than shipping a change with no real benefit. This mirrors #524's own
precedent: a measured "no win" is a valid, honestly-reported outcome, not a
failure to force through.

## Test scope

```
./scripts/test.sh -p shamir-storage
```

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-storage
```
Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's (#529's) job.

## Report format

```
[Implementation] Status: fixed / no-win-reverted / scoped-down-with-followup
  > Worker mechanics: how writes are routed, confirm reads are untouched
  > TOCTOU claim: verified true / verified false / not conclusively determined
    (with reasoning either way)
  > Bench: dataset size relative to fjall's cache layer, before/after numbers
    for BOTH writes and reads, honest characterization of the result
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-storage: pass/fail
```
