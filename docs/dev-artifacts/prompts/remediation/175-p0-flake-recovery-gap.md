# P0 flake: bounded convergence in recovery_gap_consumer_tests

Test file: `crates/shamir-server/src/sync/replication/tests/recovery_gap_consumer_tests.rs`.

## Problem (binding ruling: TEST_FRAGILITY, 2x rush reviewer agree)
The file-local `run` helper drives the follower with `with_max_iterations(1)` and zero long-poll wait, and the tests treat `run(..).await == Ok(())` as "the follower caught up". The loop counts ATTEMPTS: an empty page (replication clamps visibility to the WAL recoverability prefix `R` with a 200 ms bounded wait), a swallowed transient pull/apply/bookmark error, or a structured leader error all consume the single iteration and return `Ok(())` without advancing the bookmark. Observed once on windows CI: `explicit_persisted_gap_stops_consumer_and_supervisor_but_not_beyond_h` panicked with bookmark 1 != 2; the rerun passed.

## Required change (tests only; production follower semantics, the `R` clamp and all timeouts stay untouched)
1. Add a test helper (new file next to the tests, e.g. `converge.rs` in the same `tests/` dir, registered in `tests/mod.rs`) that:
   - takes the expected target bookmark, runs the follower for a bounded number of single-iteration attempts (e.g. at most 50, tiny `tokio::time::sleep` between attempts, each attempt itself bounded by an overall deadline helper already used in the file or a new `tokio::time::timeout`),
   - after each attempt reads the durable bookmark; returns as soon as it equals the target,
   - fails IMMEDIATELY on a returned `Err` that is terminal for the scenario (do not retry terminal results away),
   - on exhaustion panics with attempts, expected vs actual bookmark and the last observed pull outcome.
   To report the last observed pull outcome, wrap the in-process `ReplSource` in a small recording wrapper (last pull Err / structured error code+message / event versions / gap / advertised head). `run(..) == Ok(())` alone can never reveal a swallowed error.
2. Switch every successful catch-up call site to the helper (file lines at origin/master: 201 and 229 in `cold_reopen_reconciles_events_for_consumer_and_supervisor`; 211–222 supervisor-success section of the same test; 354 and 379 in `explicit_persisted_gap_stops_consumer_and_supervisor_but_not_beyond_h`; 417 in `clean_reopen_allows_follower_at_head`; keep the explicit empty-at-head assertion at 421–428).
3. KEEP strict (no retry): the terminal gap assertions (about 277 and 361). For the supervisor refusal sections (about 294–300) wait for the observable terminal state (bounded, same style) instead of assuming one attempted pull processed it.
4. Do not replace the real-journal integration with a synthetic source; do not raise any production timeout; do not bump iteration counts blindly.
5. Keep the file under 1000 lines (split into a sibling file if needed), imports at top, `mod.rs` re-exports only, JSON literals in tests multi-line.

Acceptance: `cargo clippy -p shamir-server --all-targets -- -D warnings` clean, `cargo fmt -p shamir-server -- --check` clean, layout gate clean. Tests are executed by CI only.
