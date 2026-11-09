# Failure-only diagnostics for flaky tests

Status: accepted (work in progress).

## Goal

A flaky test that fails in CI must leave enough evidence to name the mechanism from the
log alone, without a reproduction. The extra evidence is printed **only when the test
fails**; a green run prints nothing new.

## What "evidence" means

* a wall-clock timeline of the test's own steps (`mark("subscribe sent")`);
* the log records the code under test emitted during the test (the `log` facade, and
  `tracing` events where the crate uses them), kept in a bounded in-memory ring;
* for tests that can hang: a watchdog that dumps the same material before the runner's
  kill, because a killed process (nextest slow-timeout) never runs destructors;
* test-specific state that explains the failure (ports, ids, counters, the last frame
  seen, the server log tail for TS e2e).

## Rust mechanism

New dev-only crate `shamir-test-diag` (`publish = false`, never a normal dependency):

* `FlakeGuard::new(name)` — created at the top of a test, kept alive to its end.
  `mark(label)` appends to the timeline, `note(key, value)` records a state fact.
  `Drop` prints the report to stderr only when `std::thread::panicking()`.
  `FlakeGuard::with_watchdog(name, after)` additionally dumps the report once if the
  guard is still alive after `after` (chosen below the runner's slow-timeout kill), then
  keeps the test running.
* `log` capture: one process-wide `log::Log` ring (default 2000 records, all levels up to
  `Debug`) installed on first guard creation. Tests run one per process under nextest, so
  the ring is the test's own. If another logger is already installed
  (`log::set_logger` fails) the guard degrades to timeline and notes only and says so in
  the report header.
* `tracing` capture behind the feature `tracing`: a `tracing_subscriber` layer writing the
  same ring; used by crates that emit `tracing` events (server, transport-http, client).
* Output goes through `eprintln!`, so the runner shows it with the failing test only.
* Bounded: ring size and line length are capped; a green run costs one atomic check per
  record and no I/O.

Rules for adopters:

* The guard never changes a test's assertions or timing budget.
* No sleeps, no extra waits added "to catch" the failure.
* Test-only code: nothing in production paths; if a crate needs a production-side log
  line to explain the flake, it is an ordinary `log::debug!`/`trace!` at the decision point.

## TypeScript mechanism

e2e tests already carry `Timeline` and the server log (`fixture.server.logs()`).
The pattern is: collect the material in memory during the test and render it in the
thrown error message or an `afterEach` that checks the test state; nothing is printed
when the test passes. A watchdog is not needed (vitest has its own timeout and reports
the thrown/last error).

## Catalogue of tests that flaked

From CI history and checkpoints. "Fixed" means the mechanism was found and fixed; the
diagnostics stay because the class (timing, ports, OS differences) can recur.

| test | where | notes |
|---|---|---|
| `launcher_shutdown_stops_token_gc_task` | shamir-server `tests/boot/server_tests.rs` | #1802, shutdown flush stall on Windows; watchdog required (28 s stall) |
| `boot_warns_once_for_unknown_repository_factory_and_keeps_other_repositories` | shamir-db `src/shamir_db/tests/system/boot_oracle_tests.rs` | #1802, the same flush stall on Windows (5 s quiesce timeout); the test's own logger feeds the guard ring |
| `token_ws_expired_token_closes_with_4012` | shamir-e2e `tests/matrix/token/ws.rs` | fixed (#490), client sees RST instead of close code |
| `token_listener_socket_traces_redact_rest_and_native_ws_credentials` | shamir-server `tests/e2e/token_matrix/suite/observability.rs` | |
| `seed_e2e_c_incomplete_install_boot_gate_refuses_then_exact_retry_converges` | shamir-server `tests/seed_recovery_e2e/scenarios/c_refusals.rs` | |
| `restore_failure_at_pre_swap_invalidation_leaves_data_dir_untouched` | shamir-server `tests/admin/ops/backup_restore_e2e/restore_failure.rs` | |
| `copy_step_failure_propagates_and_leaves_data_dir_untouched` | shamir-server `src/tests/durability/restore_tests.rs` | Windows only: the watcher lost the race for `z_locked.txt` (a sleep is quantized to the ~15.6ms timer tick); the guard notes whether the lock was taken and the poll count |
| `http_ws_native_group` | shamir-server `tests/e2e/http_ws/native.rs` | |
| `incomplete_body_timeout_releases_declared_reservation` | shamir-transport-http `src/tests/transport_tests.rs` | macOS timing |
| `in_doubt_rename_reopens_as_exactly_one_catalogued_incarnation` | shamir-db `.../tests/catalogue/rename_restart_tests.rs` | |
| `set_null_nulls_child_fk_transactional` | shamir-db `tests/schema_ddl/fk/declarative_schema_fk_ondelete_e2e.rs` | |
| `boot_tolerates_native_validator_row` | shamir-db `tests/functions/facade/native_parity_e2e.rs` | |
| `truncation_ceiling_blocks_segment_removal_when_a5_gate_unsafe` | shamir-engine `src/tx/tests/drain/truncation/truncation_tests.rs` | |
| `recall_sq8_vs_f32_within_two_percent`, `recall_at_10_on_1k_vectors` | shamir-index `src/vector/tests/...` | statistical: log seed, dataset size, measured recall |
| e2e-login subscription test | shamir-client-ts `__tests__/connection/token/e2e-login.test.ts` | already failure-only (timeline + server log in the error) |
| browser-login "settles promptly on abort while fetch is pending..." | shamir-client-ts `src/login/__tests__/browser-login.test.ts` | timing |

A test is added to the table when it flakes; the fix for a new flake includes adopting the
guard.
