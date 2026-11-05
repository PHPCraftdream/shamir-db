# Brief: failure-only diagnostics, remaining Rust flaky tests (FLAKELOG-1b)

Read first: `docs/dev-artifacts/design/flake-failure-logs.md`, `CLAUDE.md`, and the committed crate
`crates/shamir-test-diag` (API: `FlakeGuard::new`, `with_watchdog`, `mark`, `note`).

## Deliverable

Adopt `FlakeGuard` (dev-dependency only) in these tests, as the first statement, with marks at the decisive steps
and notes for the state that explains a failure; no change to assertions, waits or budgets:

* `crates/shamir-transport-http/src/tests/transport_tests.rs`:
  `incomplete_body_timeout_releases_declared_reservation` (notes: declared length, bytes sent, timeout configured,
  reservation counters before/after; feature `tracing`).
* `crates/shamir-db/src/shamir_db/shamir_db/tests/catalogue/rename_restart_tests.rs`:
  `in_doubt_rename_reopens_as_exactly_one_catalogued_incarnation` (marks: rename started, crash point, reopen,
  catalogue contents seen).
* `crates/shamir-db/tests/schema_ddl/fk/declarative_schema_fk_ondelete_e2e.rs`:
  `set_null_nulls_child_fk_transactional`.
* `crates/shamir-db/tests/functions/facade/native_parity_e2e.rs`: `boot_tolerates_native_validator_row`.
* `crates/shamir-engine/src/tx/tests/drain/truncation/truncation_tests.rs`:
  `truncation_ceiling_blocks_segment_removal_when_a5_gate_unsafe` (notes: ceiling, gate state, segments present).
* `crates/shamir-index/src/vector/tests/lifecycle/compaction_tests/quantized.rs`
  (`recall_sq8_vs_f32_within_two_percent`) and
  `crates/shamir-index/src/vector/tests/hnsw/hnsw_adapter_tests/basic.rs` (`recall_at_10_on_1k_vectors`):
  notes: RNG seed, dataset size, k, ef, measured recall and the threshold, so a statistical miss is
  reproducible from the log alone.
* Helper code that several of these tests need goes into the crate's existing test-support module, not a copy per
  test.

Add `shamir-test-diag` to the dev-dependencies of the crates above (do not duplicate an existing entry).

## Acceptance

check/clippy/fmt green for `shamir-transport-http`, `shamir-db`, `shamir-engine`, `shamir-index`; layout gate
green; no assertion or timing budget changed; green runs print nothing new.

Final message: files changed, what each test now records, anything you could not confirm.

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
absolute paths in any file you write. Do not mention how or where CI is run. Do not hand-edit `Cargo.lock`. No
version bumps.
