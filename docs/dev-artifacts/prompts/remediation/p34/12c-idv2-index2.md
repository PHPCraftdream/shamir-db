# P34-12c - shamir-index + shamir-engine: FTS and functional indexes on identity v2, with the index2 migration (design stage S3)

Source: design `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-12-index-identity-v2-design.md` plus the P34-12 cards. Read first: `CLAUDE.md`, the design `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-12-index-identity-v2-design.md` (sections (a) scheme, (b) persisted version, (c) migration, (d) format breaks, (e) staged plan, and "Owner decisions" at the end; the design is binding), the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md` (section P34-12) and the card texts in `docs/dev-artifacts/prompts/remediation/p34/12-index-identity-v2.md`.

## Goal

Implement stage S3: FTS token identity and functional-value identity on the IDV2 codec in their own key namespaces (`FTS_V2 = 0x11`, `FUNCTIONAL_V2 = 0x12`), the envelope-version-2 rule for FTS/functional tables, and the automatic open-time index2 migration with the post-replay re-run. Closes RV-INDEX-53CEF405, L3-RV-INDEX-FBB72544 and the index2 part of L3-RV-INDEX-605EEC5D.

## Scope

Crates: `shamir-index`, `shamir-engine`. Files exactly as design section (e) S3 lists (tokenizer.rs, fts_ranked_backend.rs, fts_backend.rs, functional_backend.rs, backend.rs, posting_layout.rs, persistence.rs; engine: identity_migration.rs, write_domain_barrier.rs, read_planner.rs, tx/recovery/v2.rs).

## Acceptance

The design S3 test list is implemented and type-checks, including the RED `rv_index_fbb72544_functional_map_reversed_insertion_hits_same_posting`, `rv_index_53cef405_fts_token_identity_exact`, the rewritten goldens (`rv_index_605eec5d_golden_token_hash`, `_golden_fts_posting`, `_golden_functional_postings`, kept not deleted) and the migration tests (v1 envelope and postings seeded, crash after the flip, strict open of a v2 blob -> `UnsupportedVersion(2)`, user-scalar functional index `Failed` until `repair()`).

## Coordination with other tasks

Starts after P34-12b (S2) is merged; shares `persistence.rs`, `identity_migration.rs` and `write_domain_barrier.rs` with it.

## Decisions (binding)

Decided by the design decider (@oh) on 2026-10-30 (design, section "Owner decisions"): (OQ-1) older binaries refuse tables with FTS or functional indexes written by identity v2, downgrade is DROP INDEX or a backup, no downgrade command; (OQ-2) `blake3` with the `pure` feature, same version as `shamir-funclib`, as a direct dependency of `shamir-index`; (OQ-3) automatic blocking open-time rebuild reusing the S9b repair and the F-50 Building self-heal. The design departs from the P2 notes in two places and the design wins: no `LEGACY_INDEX_FORMAT_VERSION` 3->4 bump (a new `_m.idx.idv` marker instead), and no "verify the value on hit" stage first.

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

`cargo fmt -p <crate> -- --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Crates: shamir-index, shamir-engine (and cargo check of shamir-db, shamir-server). Report real exit codes.

## Final report

Per item of the design stage: DONE / ALREADY-FIXED / SKIPPED(decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed ones you needed; anything that needs a build or test run; every deviation from the design with its written reason.
