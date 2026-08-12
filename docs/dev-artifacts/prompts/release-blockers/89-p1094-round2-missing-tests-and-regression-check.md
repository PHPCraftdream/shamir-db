# #1094 round 2 — add the missing core watchdog tests and complete the regression check

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Round 1 (`crates/shamir-engine/src/query/batch/op_watchdog.rs`, already on
the working tree, not yet committed) delivered a sound OS-thread-based
watchdog design — `std::thread::spawn`, a lock-free `scc::HashMap`
registry, an `OpGuard` RAII wrapper correctly removing entries on every
exit path (success, error via `?`, and panic-unwind, since `Drop` always
runs on those). Independent orchestrator review confirmed the core design
is sound and confirmed `fmt`/`clippy` clean. Read
`crates/shamir-engine/src/query/batch/op_watchdog.rs` in full before
starting — it's small (~165 lines) and you need to understand its exact
API (`register_op_watchdog(alias: &str) -> OpGuard`, the `REGISTRY`
static, `WATCHDOG_WARN_THRESHOLD`/`WATCHDOG_POLL_INTERVAL` constants)
before writing tests against it.

**Two problems found in round 1 that this round must fix:**

1. **Out-of-scope edit, already reverted by the orchestrator** —round 1
   changed EVERY sibling `mod X;` declaration in
   `crates/shamir-engine/src/query/batch/tests/mod.rs` to `pub(crate) mod
   X;`, not just its own new `watchdog_tests` entry. This has already been
   reverted (the file is back to plain `mod X;` for every entry including
   `watchdog_tests`). **Do not reintroduce this** — nothing in
   `watchdog_tests.rs` needs `pub(crate)` visibility on sibling test
   modules; if you find you genuinely need it, that's a sign something
   else is wrong, not a sign to make this change again.

2. **The tests that actually matter are missing.** The original brief
   (`docs/dev-artifacts/prompts/release-blockers/88-p1094-os-thread-watchdog.md`
   — read it too) asked for 4 things under "Tests"; round 1's
   `crates/shamir-engine/src/query/batch/tests/watchdog_tests.rs` delivered
   only bookkeeping tests (`test_watchdog_initialization`,
   `test_multiple_registrations` — both just check the registry's
   insert/remove mechanics) and NONE of the behavioral ones. Specifically
   missing:
   - **The core test the whole task exists for**: stage a deliberately
     slow op and assert the watchdog's warning fires WHILE the op is
     still running, not after. Nothing in the current test suite proves
     the watchdog thread's scan-and-warn loop actually works end-to-end
     through the real call sites (`execute_plan_impl`/
     `execute_plan_tx_impl` in `batch_execute.rs`) — the existing tests
     only poke `op_watchdog.rs`'s registry API directly, never exercise
     the real batch-execution wiring.
   - A test confirming the watchdog does NOT fire for a normal, fast op
     (no false positives).
   - A test confirming a single stuck op produces exactly ONE warning,
     not one per poll interval.

## What to build

1. **Find or build a way to inject a deliberately slow op** into a real
   `execute_plan_impl`/`execute_plan_tx_impl` call. Check this crate's
   existing test infrastructure first — `executor_tests/` and the mock
   `TableResolver`/`AdminExecutor`/`FunctionInvoker` implementations used
   elsewhere in `crates/shamir-engine/src/query/batch/tests/` likely
   already have (or can be trivially extended with) a way to make one op
   `tokio::time::sleep` for a controlled duration before returning. Do
   NOT invent a parallel mocking mechanism if a working one already
   exists in this test suite — grep for how other tests in this
   directory construct a `BatchRequest`/`TableResolver` and reuse that
   shape.
2. **Core test**: drive a batch through the real `execute_plan_impl` (or
   the public entry point that calls it — check `batch_execute.rs`'s
   `pub` functions) with one op that sleeps for, say, 1.5–2 seconds
   (comfortably over `WATCHDOG_WARN_THRESHOLD` = 1s, comfortably under
   this repo's nextest per-test slow-timeout so it doesn't itself get
   flagged/killed — check `.config/nextest.toml`'s default slow-timeout,
   currently 180s, so this is generous headroom). Capture log output (use
   whatever logging-capture mechanism this codebase already uses in
   tests — grep for `log::set_logger`/`env_logger::Builder` test usage,
   or a custom `log::Log` test sink if one already exists; do not invent
   a new logging-capture crate/dependency if the workspace already has a
   pattern) and assert the "still running" warning message appears
   BEFORE the op completes — not just that it eventually appears
   somewhere in the log after the whole batch finishes. If precisely
   timing "warning fires while still running" is awkward to assert
   directly, an acceptable alternative: run the slow op with a channel/
   flag the op signals right before it finally completes, and assert the
   warning arrived (via the log sink) strictly before that signal fires.
3. **False-positive test**: a normal fast op (well under the threshold)
   through the same real call path produces NO watchdog warning.
4. **Once-only test**: a single op stuck for multiple poll intervals
   (e.g. sleep 3–4s with a 1s poll interval) produces exactly ONE
   watchdog warning for that op, not 3-4.
5. **Complete the regression check the original brief required and round
   1 started but never finished**: run
   `./scripts/test.sh -p shamir-db --full` (the FULL suite, ~2730 tests,
   with the real watchdog thread active for the whole run — no shortcuts,
   no filtering) to completion and report the real pass/fail counts.
   Round 1's session ended its turn while this was still compiling/running
   in the background, without ever seeing the result — this is the exact
   regression check this whole `#1094` investigation exists to validate
   (baseline stack safety holds for the REAL implementation, not just the
   throwaway probes). **If this run reproduces ANY
   `"has overflowed its stack"` failure or any stack-related crash, STOP
   immediately, do not attempt a fix, and report back precisely what
   failed** — this would falsify the whole investigation's premise and
   needs the orchestrator's judgment on how to proceed, not a silent
   workaround.

## Gate

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
./scripts/test.sh -p shamir-db --full
```

All four must pass clean AND actually complete (do not end your turn with
any of them still running in the background — wait for the real result).
Report the real `shamir-db --full` pass/fail/skip counts in your final
summary; "tests pass" without the actual numbers is not acceptable given
this is the specific check this task has been building toward.

## Also worth a quick look (not mandatory, use judgment)

`op_watchdog.rs`'s watchdog-thread loop does a two-pass update for the
`already_warned` flag (collect IDs into a `Vec` during `iter_sync`, then a
second `read_sync`+`update_sync` pass per ID). This works, but if you
notice while writing tests that this shape can be simplified (e.g. by
updating the flag directly inside the `iter_sync` closure, if `scc`'s API
allows mutation during iteration — check its docs/other call sites in
this codebase) feel free to simplify, but this is NOT the priority for
this round — the missing tests and the incomplete regression check are.
Don't let a refactor here distract from delivering those.
