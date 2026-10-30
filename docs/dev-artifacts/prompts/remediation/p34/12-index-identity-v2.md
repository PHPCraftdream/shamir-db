# P34-12 - shamir-index: persisted hash identity ("index identity v2"), design first

Source: P3/P4 revalidation (master `07bc804c`) plus the P2 carry-overs. Read first: `CLAUDE.md`, `crates/shamir-index/KNOWN_LIMITATIONS` notes (grep `identity`), the cards below in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` / `P3.md`, and the P2 decisions recorded in `docs/dev-artifacts/prompts/remediation/p2/06-index-fts-hash.md`.

## Goal

Design, then implement, a persisted hash identity ("index identity v2") that removes the insertion-order dependence of functional Map hashing, the false DuplicateKey risk of composite numeric unique indexes and the FxHasher-stability coupling, with an explicit migration or rebuild policy for existing persisted indexes. Design first: this stage needs a written design and an owner decision on any incompatible persisted-format change before code.

## Phases

1. **Design (read-only reviewer, no code).** Deliver a written design: (a) which hash/identity scheme replaces the current persisted one for FTS posting keys, functional Map hashing and unique-index keys, and why it is stable across platforms and dependency versions; (b) how a persisted identity version is recorded so old binaries and new binaries can tell them apart; (c) the migration path for existing persisted indexes (in-place rewrite, background rebuild from source, or refuse-and-rebuild) and its cost; (d) whether any step breaks the persisted format without a safe migration; (e) the test plan, including reversed-insertion Map lookups, composite numeric unique keys and golden vectors. File: `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-12-index-identity-v2-design.md`.
2. **Owner decision.** Any incompatible persisted-format change, new dependency or rebuild policy the design contains goes to the owner as a numbered question before code.
3. **Implementation** as a normal stage (executor, reviewer, CI) from the approved design; this brief is extended with the decisions before the executor is launched.

## Acceptance

- A written design (read-only reviewer) covering: the identity scheme, versioning of persisted hashes, the migration or rebuild path, the behaviour of old binaries, and the test plan, before any code.
- Owner decision recorded for any incompatible persisted-format change.
- After implementation: reversed-insertion Map lookups hit the same posting, composite numeric unique indexes raise no false DuplicateKey, golden vectors cover the new identity.

## Coordination with other tasks

Wave 3; starts after P34-07 are merged. tokenizer.rs and model/meta/persistence.rs are shared with P34-07 (0695AADA, CF86432E).
- 605EEC5D and FBB72544 were SKIPPED in P34 decisions as deferred to this stage; 53CEF405 and 9E472725 are P2 carry-overs.

## Findings

### L3-RV-INDEX-605EEC5D - Persisted posting keys depend on FxHasher output stability, with no version coupling and a caret-pinned dependency

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted risk, deferred to the scheduled 'index identity v2' follow-up (P2 decisions for 53CEF405/9E472725/605EEC5D/FBB72544). Golden vectors (fts/tokenizer_tests.rs:50-55 and siblings), persisted-identity doc comments (fts/tokenizer.rs:429-434, backends/functional_backend.rs:150-152) and KNOWN_LIMITATIONS.md:416-425 are in master. Cargo.lock pins rustc-hash 2.1.2 and the golden tests fail CI on drift. A portable identity with a persisted algorithm marker needs a format bump (LEGACY_INDEX_FORMAT_VERSION 3->4 plus an index2 identity marker with rebuild), which belongs to identity v2, not to this sprint. Per P2, do not add an exact rustc-hash pin.
- Evidence at `07bc804c`: crates/shamir-index/src/tests/fts/tokenizer_tests.rs:50-55; crates/shamir-index/src/fts/tokenizer.rs:429-434; crates/shamir-index/Cargo.toml:36; crates/shamir-index/src/model/meta/persistence.rs:32-37
- Recipe: Define portable persisted hash identity and explicit migration/rebuild rules in index identity v2.
- Verifier note: 64-bit golden outputs help, but caret FxHasher and absent persisted algorithm/target identity leave compatibility policy unresolved.

### L3-RV-INDEX-FBB72544 - FunctionalBackend Map hashing is insertion-order dependent (byte-identity floor violation)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Deferred to index identity v2 (P2 decision for RV-INDEX-FBB72544). Map hashing in functional_backend.rs:182-189 depends on insertion order. Its bytes are pinned by rv_index_605eec5d_golden_functional_postings (functional_backend_tests.rs:393-402), and KNOWN_LIMITATIONS.md:420-422 documents DROP + CREATE as the remedy. index2 has no identity-version gate, so canonicalizing now would silently break lookups on existing functional indexes. Fix together with 605EEC5D under the identity-v2 version bump, and update the Map golden vector in the same change.
- Evidence at `07bc804c`: crates/shamir-index/src/backends/functional_backend.rs:182-189,243-255,346-356
- Recipe: Canonicalize recursive Map identity with a versioned persisted-index migration and reversed-insertion lookup parity tests.
- Verifier note: Reachable functional map keys still hash insertion order; recursive canonical identity needs persisted-version migration.

## Carried over from the P2 sprint

### RV-INDEX-53CEF405 - FTS posting keys hash untrusted token text with unkeyed `FxHasher` (`token_hash`)

- Sprint state: DEFERRED: persisted hash identity redesign, only golden vectors + KNOWN_LIMITATIONS.
- Risk `medium`; fix size/kind `L/decision`.
- Evidence: crates/shamir-index/src/fts/tokenizer.rs:447-452; crates/shamir-index/src/backends/fts_ranked_backend.rs:136-158
- Recipe: Choose collision-resolving token identity with an explicit persisted FTS rebuild/version policy.
- Verifier note: Document and query paths use hash-only token identity; tokenizer-valid chosen-collision exploitation is not demonstrated.

### RV-INDEX-9E472725 - Regular + unique index keys use two correlated FxHasher streams as "collision resistance"; unique constraints are enforced on hash alone

- Sprint state: DEFERRED: persisted hash identity redesign (strongest risk: false DuplicateKey on composite numeric unique indexes).
- Risk `medium`; fix size/kind `L/decision`.
- Evidence: crates/shamir-index/src/base_index/infra/index_keys.rs:227-258; crates/shamir-index/src/base_index/index_manager_unique/validate.rs:310-345; Cargo.lock:3030
- Recipe: Choose collision-resolving canonical-value identity and an index migration/rebuild policy.
- Verifier note: Correlated dual hashing remains the sole value identity; unique conflicts inspect RecordId rather than canonical values.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Final report

Phase 1: the design document path and its open owner questions. Phase 3: per card DONE / DISPUTED with evidence, tests added and the old-code failure they prove.
