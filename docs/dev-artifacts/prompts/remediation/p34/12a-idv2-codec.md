# P34-12a - shamir-index: identity v2 codec and versioned envelope (design stage S1, no wiring)

Source: design `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-12-index-identity-v2-design.md` plus the P34-12 cards. Read first: `CLAUDE.md`, the design `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-12-index-identity-v2-design.md` (sections (a) scheme, (b) persisted version, (c) migration, (d) format breaks, (e) staged plan, and "Owner decisions" at the end; the design is binding), the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md` (section P34-12) and the card texts in `docs/dev-artifacts/prompts/remediation/p34/12-index-identity-v2.md`.

## Goal

Implement stage S1 of the design: the frozen canonical identity codec, the inline/digest split with the BLAKE3 locator, the unique-posting value helpers, the marker constants and the envelope helpers `new_with_version` / `open_accepting`. Nothing is wired into any index yet; no persisted byte changes.

## Scope

Crates: `shamir-index`. Files exactly as design section (e) S1 lists: `crates/shamir-index/Cargo.toml` (blake3), `src/model/mod.rs`, new `src/model/identity/` (mod.rs, canonical.rs, identity_bytes.rs, markers.rs), `src/model/meta/meta_envelope.rs`, new `src/tests/identity/`. `Cargo.lock` changes only for the new direct dependency edge (blake3 is already locked). Folder limits: `src/model/` has 6 entries, `src/tests/` has 5.

## Acceptance

The design S1 test list is implemented and type-checks (`p34_12_codec_golden_vectors` with literal hex and no pointer-width gate, map reversed-insertion parity, set order parity and dedup, numeric class parity, prefix-free corpus, scalar-ref/inner-value parity, inline/digest boundary at 256/257, the 9E472725 witness pair distinct, envelope open_accepting and strict open rejecting v2). Items are `pub` crate API so nothing is dead code; no `#[allow]`.

## Coordination with other tasks

Runs in parallel with P34-07 (shamir-index). S1 only adds files plus `meta_envelope.rs`; do not touch `fts/tokenizer.rs`, `model/meta/persistence.rs`, `backends/` or `base_index/`.

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

`cargo fmt -p <crate> -- --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Crates: shamir-index. Report real exit codes.

## Final report

Per item of the design stage: DONE / ALREADY-FIXED / SKIPPED(decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed ones you needed; anything that needs a build or test run; every deviation from the design with its written reason.
