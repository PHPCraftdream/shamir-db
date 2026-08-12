# #1094 — OS-thread-based live watchdog for stuck batch ops

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background — read this before writing any code

`crates/shamir-engine/src/query/batch/execution_deadline.rs`'s module doc
has a long "#1085 investigation" section (search for that heading) — read
it in FULL first. Summary: three prior same-session attempts at an
in-process (`tokio::select!`/`tokio::spawn`) watchdog all reproduced
`"has overflowed its stack"` on Windows test binaries, even a version that
added essentially zero stack-frame size to the hot path. A follow-up
investigation (same doc, three dated "same-day probe" entries) measured
actual stack headroom via `GetCurrentThreadStackLimits` and found:

- Both a plain nextest-spawned test thread and a `tokio::test(flavor =
  "multi_thread")` worker get the normal Rust `std::thread` default of 2
  MiB, with ~99.8%+ free at a shallow measurement point.
- A 200 KiB threshold-trip probe placed inside `execute_single_impl`
  (`query_runner.rs`) — panicking (a normal nextest FAIL) if ANY op
  execution observes less than 200 KiB of headroom — was run across the
  FULL `shamir-db` + `shamir-engine` suite (2730 tests, `--full`). It
  **never fired, not once**. The 18 unrelated failures in that run were a
  pre-existing missing-WASM-SDK-path environment issue, nothing to do with
  stack depth.
- **Conclusion: baseline stack usage under normal execution is ample
  everywhere in both suites.** The three prior overflows were caused by
  the SIZE of the wrapper machinery itself (a `select!`-raced pinned
  future, or a `tokio::spawn`'s own task-state overhead), not by call
  depth already sitting near the edge. This is why an **OS-thread-based**
  watchdog — one that never touches the async call graph's own stack at
  all — is the recommended design, and is now reasonably premised.

## The goal (unchanged from the original ask)

A live diagnostic watchdog that logs `"batch op 'alias' still running
after Ns"` WHILE a single op is stuck inside its own `.await`, closing the
gap `execution_deadline.rs`'s "Deliberate non-goal" section documents:
`ExecutionDeadline`'s cooperative checkpoints and the existing
post-completion `warn_if_op_slow` (`batch_execute.rs:31`) both only
notice/log an overrun AFTER the stalled op returns — an op that never
returns produces NO signal at all today. **Strictly additive/observational
— this must NEVER cancel or otherwise affect the op's execution.**

## The two call sites

`crates/shamir-engine/src/query/batch/batch_execute.rs`, inside
`execute_plan_impl` (~line 395) and `execute_plan_tx_impl` (~line 521) —
both have the IDENTICAL shape:

```rust
let op_started = Instant::now();
let result = execute_single_impl(alias, entry, resolver, admin, invoker,
    &resolved_refs, actor, db_name, depth, params, result_encoding, deadline)
    .await?;
warn_if_op_slow(alias, op_started.elapsed());
```

Confirmed (read the surrounding loop yourself to verify): ops within one
`execute_plan_impl`/`execute_plan_tx_impl` call execute strictly
SEQUENTIALLY (`for stage in &plan.stages { for alias in stage { ...
.await... } }` — no concurrent dispatch within one batch), so at most ONE
op is in-flight per active batch-execution call at a time. Multiple
DIFFERENT batch executions (different connections/requests) can be
in-flight concurrently across the process, though — the design must
support that (see below), not assume global single-op state.

## Design (recommended shape — verify and adjust based on your own reading, but do not reintroduce any in-process technique this doc's investigation already ruled unsafe)

1. **Registry**: a `scc::HashMap<u64, (String, Instant), THasher>` (per
   this repo's `CLAUDE.md` concurrency conventions — lock-free concurrent
   map, `THasher` default), keyed by a unique per-in-flight-op id (e.g. an
   `AtomicU64` counter, or `Instant::now()`'s pointer address of a local —
   pick whichever is simplest and collision-free), storing `(alias,
   started_at)`. Lives in a `OnceLock`/static, or threaded through
   `ExecutionDeadline`/a new small struct if that's cleaner given the
   existing plumbing — your call, but it must be reachable from BOTH call
   sites above without invasive signature changes elsewhere.
2. **Registration**: immediately before each `.await` on
   `execute_single_impl`, insert `(alias.to_string(), op_started)` into the
   registry under a fresh id; immediately after the `.await` resolves
   (success OR error — use a scope guard / `Drop` impl or explicit
   removal on both paths, do not leak entries on early `?` returns),
   remove that entry. This is a couple of lock-free map ops on the async
   hot path — negligible cost, and it's exactly the kind of state this
   task's own investigation confirmed is safe (small, not stack-resident
   wrapper state — a heap-allocated map entry, not a stack frame).
3. **Watchdog thread**: ONE `std::thread::spawn` (not `tokio::spawn` — it
   must run OUTSIDE the async runtime entirely, per the investigation's
   conclusion) started once, lazily, on first use (or eagerly at process
   start if there's a clean existing init hook — check `shamir-db`/
   `shamir-server`'s startup sequence for one; do not invent a new global
   init mechanism if an existing one fits). Loop: sleep some interval
   (e.g. 1s — reuse or reference `SLOW_OP_WARN_THRESHOLD` from
   `batch_execute.rs` for consistency, don't invent an unrelated
   constant), then scan the registry (`scc::HashMap::retain`/iteration —
   check the crate's existing lock-free iteration patterns, e.g.
   `IndexManager`'s use of `scc::HashMap` elsewhere in this codebase, for
   the idiomatic form) and `log::warn!` for any entry whose `elapsed()`
   exceeds a threshold (again reuse/reference `SLOW_OP_WARN_THRESHOLD`,
   or a related-but-distinct "still running" threshold if you judge the
   semantics warrant a different number — justify the choice either way).
   **Log once per stuck op, not on every scan interval** — track "already
   warned" state (e.g. a third field in the map value, or a separate
   small set) so a genuinely stuck op doesn't spam the log every second
   for its whole stuck duration; one warning per op is enough to close the
   observability gap.
4. **No cancellation, no interruption of any kind** — the watchdog thread
   NEVER touches the op's own execution, never sends a cancel signal,
   never panics the op's task. It only reads the registry and logs.
5. **Shutdown**: does the watchdog thread need a clean shutdown path (for
   graceful process exit, or test-binary teardown)? Investigate whether
   this matters for this codebase's process lifecycle (check how other
   long-lived background threads/tasks in this codebase, if any, handle
   shutdown — e.g. search for existing `std::thread::spawn` call sites
   workspace-wide for precedent) and either wire in a clean shutdown or
   document why a daemon-style thread that just dies with the process is
   acceptable here (likely fine for a diagnostic-only thread with no
   owned resources needing flush/cleanup, but confirm rather than assume).

## What NOT to do (already tried, already failed — see the investigation doc)

- Do NOT use `tokio::select!` racing a pinned future at the call site.
- Do NOT use `tokio::spawn` for the watchdog itself, even with a minimal
  calling-frame footprint — this reproduced the SAME broad overflow as
  the `select!` approach in the prior investigation, for reasons not
  fully understood (see the doc) even though the added frame size was
  supposedly negligible. The watchdog's execution context must be a
  genuine OS thread (`std::thread::spawn`), full stop.
- Do NOT add any additional state to `execute_single_impl`'s or
  `execute_plan_impl`'s/`execute_plan_tx_impl`'s own stack frames beyond
  a cheap map insert/remove (a heap op, not a stack-resident wrapper).

## Tests

1. A test that stages a deliberately slow op (reuse whatever existing
   test infrastructure this codebase has for injecting delay — check
   `execution_deadline.rs`'s own test suite and `batch_execute.rs`'s
   tests for a pattern, e.g. a mock resolver/executor that sleeps) and
   asserts the watchdog's warning fires WHILE the op is still running
   (not after) — this is the core behavior the whole task exists for;
   a test that only checks post-completion behavior proves nothing new.
2. A test confirming the watchdog does NOT fire for a normal, fast op
   (no false positives).
3. A test confirming a single stuck op produces exactly ONE warning, not
   a warning per scan interval, if you implement the once-only dedup from
   design point 3.
4. **Reuse this investigation's own threshold-trip verification
   technique as a mutation/regression check**, per
   `execution_deadline.rs`'s own recorded suggestion: temporarily lower
   the watchdog's poll interval or threshold to something aggressive and
   confirm the FULL `shamir-db` + `shamir-engine` `--full` suite (2730
   tests) still passes clean with the real (non-mocked) watchdog thread
   running for real during every test — this is the actual regression
   check that the stack-safety conclusion holds for the REAL
   implementation, not just the throwaway probe. Report the pass/fail
   count. If this run reproduces ANY overflow, STOP — do not paper over
   it, report back with the exact failure and do not attempt a fix
   without conferring on the approach (this would falsify the whole
   investigation's premise and needs escalation, not a silent tweak).

## Gate

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
./scripts/test.sh -p shamir-db --full
```

All four must pass clean, PLUS the full-suite regression run from Tests
item 4 above (2730 tests, real watchdog thread active for the whole run)
must show zero overflow-related failures — report the actual pass/fail
counts in your final summary, not just "tests pass".
