# Brief: failure-only diagnostics, core crate and shamir-server / shamir-client tests (FLAKELOG-1)

Read first: `docs/dev-artifacts/design/flake-failure-logs.md` (the design and the catalogue), `CLAUDE.md`,
`crates/shamir-index/src/tests/support/log_sink.rs` (an existing test log sink, for style only).

## Deliverable

1. New dev-only crate `crates/shamir-test-diag` (`publish = false`; edition and lints as the other crates;
   the workspace glob picks it up). Layout: `src/lib.rs` (re-exports), `src/guard.rs` (`FlakeGuard`),
   `src/ring.rs` (bounded line ring), `src/log_capture.rs` (process-wide `log::Log` into the ring, installed on
   first guard, degrades gracefully when a logger already exists), `src/tracing_capture.rs` behind feature
   `tracing` (a `tracing_subscriber` layer into the same ring), `src/tests/` (unit tests per the repo layout:
   `tests/mod.rs` manifest + topical files).
2. API (as in the design): `FlakeGuard::new(name)`, `with_watchdog(name, after)`, `mark(label)`,
   `note(key, value)`; `Drop` prints the report to stderr only when `std::thread::panicking()`; the watchdog
   dumps once if the guard is still alive after `after`, from a detached thread that holds only shared state,
   never prevents process exit, and never panics. Report = header (name, elapsed, logger installed or degraded),
   timeline, notes, ring contents (oldest first, capped).
3. Unit tests for the crate: green path prints nothing (inject a sink instead of stderr so this is assertable),
   panic path prints timeline + ring, ring bound is respected, watchdog dumps once and stops, degraded mode.
4. Adopt the guard in these existing tests, as the first statement, with marks at the decisive steps of each test
   (no change to assertions, waits or budgets):
   * `crates/shamir-server/src/tests/boot/server_tests.rs`: `launcher_shutdown_stops_token_gc_task` (watchdog;
     choose the duration from the slow-timeout in `.config/nextest.toml` so the dump comes before the kill; note
     the shutdown stage timings that already go to `log`), plus any sibling shutdown/boot test in that file that
     waits on a shutdown.
   * `crates/shamir-server/tests/e2e/token_matrix/suite/observability.rs`:
     `token_listener_socket_traces_redact_rest_and_native_ws_credentials` (feature `tracing`).
   * `crates/shamir-server/tests/seed_recovery_e2e/scenarios/c_refusals.rs`:
     `seed_e2e_c_incomplete_install_boot_gate_refuses_then_exact_retry_converges`.
   * `crates/shamir-server/tests/admin/ops/backup_restore_e2e/restore_failure.rs`:
     `restore_failure_at_pre_swap_invalidation_leaves_data_dir_untouched` (and its lib twin in
     `crates/shamir-server/src/tests/durability/restore_tests.rs` if it is the same scenario).
   * `crates/shamir-server/tests/e2e/http_ws/native.rs`: the `http_ws_native_group` scenario.
   * `crates/shamir-client/tests/matrix/token/ws.rs`: `token_ws_expired_token_closes_with_4012` (marks: connect,
     token expiry, first frame/close/reset observed, the code or error received).
5. Add `shamir-test-diag` as a `[dev-dependencies]` entry only (path dependency) of each crate you adopt it in.
6. Update the workspace section of `CLAUDE.md` (crate count 30 -> 31 and the crate list) with a one-line mention;
   update `scripts/ci/matrix/affected_crates.py` or other crate lists only if the repo requires every crate to be
   registered there (read them and say what you found).

## Acceptance

A panicking test prints the report, a passing test prints nothing; every listed test has the guard with
meaningful marks; check/clippy/fmt green for `shamir-test-diag`, `shamir-server`, `shamir-client`; layout gate
green; no assertion or timing budget changed.

Final message: files changed, what each adopted test now records, anything you could not confirm.

## Policy (verbatim, mandatory)

**Compilation, builds and tests run on GitHub CI, not on this machine.** Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc/vitest builds and test runs,
e2e/integration harnesses, any release or full-workspace build.

Allowed locally, scoped to the crates you touched (`-p <crate>`): `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and
`cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py` (it reads tracked files only: the
orchestrator stages before the final check). Use a `CARGO_TARGET_DIR` outside the repository. Never push code that
fails check, clippy or fmt. Read every changed file for the whole class of error (module paths, imports, privacy,
`cfg(test)` gating, unused items). A child that believes it needs a build or test run says so in its final message.

> ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
> working tree or index. Only edit files; the orchestrator commits.

Layout: at most 7 entries per folder, at most 1000 lines per file; `mod.rs` re-exports only; no inline
`#[cfg(test)]` modules; imports at the top; no visibility widening and no new `#[allow]`. No machine-local
absolute paths in any file you write. Do not mention how or where CI is run. Do not hand-edit `Cargo.lock` (the
orchestrator regenerates it). No version bumps.
