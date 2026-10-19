# FLAKE-2 — Windows clean-shutdown flush stalls (`clean shutdown timed out during flush`)

## Observed (CI evidence, Windows `cargo test lib` only; Ubuntu/macOS never)

- master run 39201929216 (merge of #197): `shamir-db shamir_db::tests::durability_levels_tests::buffered_batch_executes_successfully` FAIL after 38.8 s, panicked at `durability_levels_tests.rs:124:30` on `shamir.flush_all().await.unwrap()`: `Storage("unclean shutdown: [(\"__system__/system\", \"Storage backend error: clean shutdown timed out during flush\")]")`.
- PR #202 full matrix run 39236588612: TWO tests failed at the same moment with the identical message, 19.4 s each, adjacent nextest indices 418/419: `durability_levels_tests::buffered_batch_executes_successfully` and `durability_policy_tests::boot_reopen_inherits_policy_and_recovered_prefix` (`durability_policy_tests.rs:231`, flush_all again).
- Both tests: `ShamirDb::init(SystemStoreConfig::Fjall(path))`, one extra fjall repo, a couple of rows, then `flush_all()`; budget = `COMMIT_QUIESCE_TIMEOUT` = 15 s (`shamir-db/src/shamir_db/shamir_db/db_management.rs:20`), shared by `flush_all_report` over all repos + the system repo (`RepoInstance::shutdown_clean`, `shamir-engine/src/repo/clean_horizon/shutdown_impl.rs`: lifecycle drain -> `quiesce_commits` -> `flush_buffers` -> `finish_clean_shutdown`, all under one absolute deadline). The failing stage is the `flush_buffers` one of the SYSTEM repo.
- Two different tests failing at the same wall-clock second says: a process-/machine-wide stall (disk flush under load) or something global the two tests share, not a test-local race.

## Task

1. Read the whole shutdown path for a Fjall repo and write down (in the final message) the exact sequence of blocking/fsync-class operations `flush_buffers` performs for a repo with one table (`drainer().drain_all` incl. truncation/clean-horizon checkpoint, `tx_info_store().flush_drain`, per-table data/info `flush_drain`, interner `persist`, the single backend `persist_once`) and which of them are redundant or repeated on the same journal.
2. Find any blocking call executed ON an async runtime thread (std::fs, fjall sync API, `std::thread::sleep`, `Mutex::lock` held across a slow call) in that path or in `Repo::persist_once`/`flush_drain` of the Fjall backend (`shamir-storage`): in a `#[tokio::test]` current-thread runtime a blocking call stalls the runtime and its timers, and the `timeout_at` then reports a timeout although the work eventually finished.
3. Fix the ROOT: move blocking work to `spawn_blocking`, coalesce redundant fsyncs, avoid persisting the same journal twice - whatever the reading proves. NEVER raise `COMMIT_QUIESCE_TIMEOUT` or any test timeout, never add retries that hide the stall. If, after reading, you prove the path is already minimal and the stall is the runner's disk, say so with file:line evidence for every step and propose (do not apply) the test-side change.
4. Add per-stage timing diagnostics to `RepoInstance::shutdown_clean`/`flush_buffers`: a `log::warn!` naming the repo and the stage whenever one stage exceeds 1 s (no hot path involved; shutdown only).
5. Add a `workflow_dispatch`-only workflow `.github/workflows/flake-loop.yml` (windows-latest, ubuntu-latest) that runs `./scripts/test.sh -p shamir-db -- durability_levels_tests durability_policy_tests` 30 times in a loop with the full log kept (each iteration's exit code and the nextest slow/fail lines echoed, the job fails if any iteration failed). It is the CI loop the project rules prescribe for flakes; the orchestrator triggers it.

## Rules

Standard policy of this repository applies (see the policy text at the top of this brief): no local builds or tests; only `cargo check/clippy/fmt -p <crate>` and `python -I scripts/ci/check_layout.py`. Layout limits (<=7 entries per folder, <=1000 lines per file) are a ratchet. Imports at the top; `mod.rs` re-exports only.
