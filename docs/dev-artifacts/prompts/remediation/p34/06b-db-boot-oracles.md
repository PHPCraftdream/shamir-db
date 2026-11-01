# P34-06b - shamir-db: boot diagnostics and catalogue-cost oracles (RV-DB-A7A29B02-FACET, RV-DB-3EB43AC7, RV-DB-23A03103)

Source: the cards in `docs/dev-artifacts/prompts/remediation/p34/06-db-engine.md` (A7A29B02-FACET, 3EB43AC7, 23A03103) and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`. Read first: `CLAUDE.md`, `crates/shamir-db/src/shamir_db/shamir_db/core.rs` (the boot loop that registers repositories, search for `factory_from_meta` and the `log::warn!` for a skipped repository), `crates/shamir-db/src/shamir_db/system_store/store.rs` and `catalogue.rs`.

## Goal

The two code changes of P34-06 shipped without a behavioural oracle: the boot warning for a persisted repository without an available backend factory (A7A29B02-FACET) and the single-pass grouping of table catalogue records by repository (3EB43AC7) are covered only by review. Add oracles that fail on the old behaviour.

## Binding behaviour

1. A7A29B02-FACET: booting over a system catalogue that holds a repository whose backend has no factory logs ONE warning for that repository carrying its database name, repository name, backend (engine) and the reason, and the other repositories still register. Build the fixture with a durable system store in a temporary directory (the fjall system store if the feature is on in the tests, otherwise the cheapest durable seam that already exists in the shamir-db tests): create a database and a repository, rewrite its persisted catalogue row to an engine name no factory knows, re-open, and capture the log. Capture with a `log::Log` implementation installed as the global logger and a static, mutex-protected message list; the precedent is `mem_policy_warning_is_once_per_open_not_per_commit` in `crates/shamir-db/src/shamir_db/shamir_db/seed/tests/durability_policy_tests.rs`. The CI test runner (nextest) runs every test in its own process, so the global logger is safe; keep the "only assert when this process installed the logger" guard that the precedent uses so that a plain `cargo test` run does not fail spuriously.
2. 3EB43AC7: the boot path must touch every table catalogue record once, not once per repository. Extract the grouping loop of `core.rs` into a small pure helper in its own file (one primary export, no visibility widening beyond `pub(super)` for a NEW item) that the boot loop calls, and give the helper an observable cost: a test builds R repositories x T tables of catalogue records and asserts that the number of record visits is T (or R+T), not R x T, with a counter that is a parameter of the helper (a closure or a `&mut usize`), not a global. The registration-before-recovery order of the boot loop must not change.
3. 23A03103 stays as it is: the orchestrator verified (2026-11-01) that system tables are created without indexes (`system_store/store.rs`, `TableConfig::new` without `with_indexes`) and that no composite-key keyed read exists, so the filtered read in `load_repository` / `load_repository_record` with its corrupt-record errors remains. Add ONE comment line at `load_repository_record` stating that the lookup is a filtered read over an index-less catalogue table and why (corruption errors are preserved); report the card as SKIPPED(decision) with this evidence.

## Scope

Crate `shamir-db`: `crates/shamir-db/src/shamir_db/shamir_db/core.rs` (extract the helper only), one new helper file next to it, the system store comment, tests under `crates/shamir-db/src/shamir_db/tests/` (new topical files, wire them without widening visibility; keep every folder at 7 entries or fewer and every file under 1000 lines, note that `access_meta_tests.rs` is exactly at the line limit and must not grow).

## Coordination with other tasks

Nothing else edits these files.

## Decisions

23A03103: SKIP (accepted, see above). The other two cards are implemented as described.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p <crate> -- --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Crates: shamir-db. Report real exit codes.

## Final report

Per item of the design stage: DONE / ALREADY-FIXED / SKIPPED(decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed ones you needed; anything that needs a build or test run; every deviation from the design with its written reason.
