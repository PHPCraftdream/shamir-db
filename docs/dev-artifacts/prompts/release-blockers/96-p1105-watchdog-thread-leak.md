# #1105 CRITICAL — op_watchdog.rs leaks one OS thread per batch op (#1094 regression)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Found by a final adversarial review of the whole session's accumulated work
(commit range `b556913f..HEAD`). Confirmed by direct code reading (not just
the review's own claim) before this brief was written.

File: `crates/shamir-engine/src/query/batch/op_watchdog.rs`.

`init_watchdog()` (lines ~92-142) is meant to spawn exactly ONE OS thread for
the process lifetime — the module doc says so explicitly ("ONE
`std::thread::spawn` started lazily on first use", "exactly one thread").
But the actual code:

```rust
fn init_watchdog() {
    let registry = REGISTRY.get_or_init(|| Arc::new(HashMap::with_hasher(THasher::default())));
    let registry_for_thread = Arc::clone(registry);

    let handle = std::thread::spawn(move || {
        loop { /* sleep 1s, scan registry, log::warn! on stuck ops */ }
    });

    // Ignore the result if another thread beat us to initialization.
    let _ = WATCHDOG_THREAD.set(handle);
}
```

`std::thread::spawn` runs **unconditionally**, every time `init_watchdog` is
called. The `OnceLock` dedup only happens AFTER the spawn, at
`WATCHDOG_THREAD.set(handle)` — and `OnceLock::set` failing on the 2nd+ call
only drops the returned `JoinHandle`, which **detaches** the already-running
thread rather than preventing its creation. The detached thread keeps running
its `loop { sleep(1s); scan; }` forever.

`init_watchdog()` is called from `register_op_watchdog(alias: &str)`
(line ~155) on **every single call**, and `register_op_watchdog` is called
from `batch_execute.rs:397` and `:524` — i.e. on every op of every batch,
both the transactional and non-transactional paths.

**Concrete failure scenario**: a server processing 1,000,000 batch ops over
its lifetime accumulates ~1,000,000 live, leaked OS threads, each holding a
reserved stack, each waking once per second to scan the shared registry and
call `log::warn!` on any op whose `elapsed()` exceeds the 1s threshold. A
single stuck op can now produce up to N duplicate warnings (N = number of
leaked threads), since `already_warned` is observed/updated independently by
each thread's own scan pass, not globally coordinated. Well before reaching
anywhere near 1M threads, this exhausts OS thread-handle limits, reserved
stack memory, or scheduler capacity. Additionally, every `register_op_watchdog`
call now pays real OS-thread-spawn latency (measured ~60µs per call in a
throwaway probe, vs. ~100ns for the `scc::HashMap::insert_sync` alone) — a
latency tax on every batch op today, not just a slow future leak.

Existing tests (`crates/shamir-engine/src/query/batch/tests/watchdog_tests.rs`)
do not catch this because `cargo nextest` runs each test in its own process,
and every existing test drives a **single-op** batch — so exactly one thread
exists per test process regardless of the bug. The bug is only observable
when `register_op_watchdog` is called more than once within the same
process.

## Fix

Use `OnceLock::get_or_init` on `WATCHDOG_THREAD` itself, so the spawn closure
is guaranteed by `OnceLock`'s own semantics to run **at most once** — no
thread is ever created and then discarded/detached:

```rust
fn init_watchdog() {
    WATCHDOG_THREAD.get_or_init(|| {
        let registry = REGISTRY.get_or_init(|| Arc::new(HashMap::with_hasher(THasher::default())));
        let registry_for_thread = Arc::clone(registry);

        std::thread::spawn(move || {
            loop {
                // ... existing loop body, unchanged ...
            }
        })
    });
}
```

(Adjust exactly to fit the existing code structure — the point is: the
`std::thread::spawn` call must live INSIDE the `get_or_init` closure, not
before it, so it only ever executes on the winning racer.)

Do not otherwise change the watchdog's behavior (poll interval, warning
threshold, registry structure, `OpGuard`/`Drop` semantics) — this is a
scoped fix to the thread-creation race, nothing else in this module needs to
change.

## Required test

Add a regression test that calls `register_op_watchdog` (or exercises the
codepath that calls it, e.g. driving a multi-op batch through
`execute_single_impl`) **multiple times within the same test process**, and
asserts only one watchdog thread was ever spawned. A reasonable approach:
add a test-only counter (e.g. an `AtomicUsize` incremented once inside the
`get_or_init` closure, gated `#[cfg(test)]` or otherwise scoped so it doesn't
leak into production behavior) and assert it equals 1 after multiple
`register_op_watchdog` calls. The test MUST fail on the current (buggy) code
and pass after the fix — prove this by temporarily reverting the fix locally,
confirming the new test goes red, then restoring the fix and confirming green
again (mutation test), before finalizing.

Place the new test in the existing
`crates/shamir-engine/src/query/batch/tests/watchdog_tests.rs` file, following
this project's test-organization conventions (see `CLAUDE.md`'s "Test
organisation" section — one `tests/` dir, split by topic, `tests/mod.rs` is a
manifest only).

## Gate

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

All three must pass clean. Report: did the new test genuinely fail before
the fix and pass after (mutation-tested)? Real gate pass/fail counts, not a
paraphrase.
