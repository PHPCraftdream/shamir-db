# P34-07c - shamir-index: remove the unused `VectorBackendRef::External` variant (RV-INDEX-CF86432E)

Source: card L3-RV-INDEX-CF86432E (external vector-backend API key persisted in cleartext inside the index-metadata blob) and the owner's question of 2026-11-01: "do we ourselves call the vector API? can it be moved to the client? it looks like it is not our business". Read first: `CLAUDE.md`, `crates/shamir-index/src/model/kind.rs` (`VectorBackendRef`), `crates/shamir-index/src/model/meta/persistence.rs` (the persisted index metadata envelope), `crates/shamir-index/src/tests/model/kind_tests.rs` (the `External` redaction test), `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_index_admitted.rs` (the only constructor, `InProcessHnsw`).

## Verified facts (orchestrator, 2026-11-01; re-verify each with `git grep` before you rely on it)

- `VectorBackendRef::External { driver, url, api_key_secret: SecretString }` is never constructed outside tests: the engine builds only `InProcessHnsw { ef_construct: 200, m: 16 }` (`write_domain_index_admitted.rs:316`), the wire operation `CreateIndexOp` carries only dim, metric and quantization (no backend, url or key), and `git grep -n "VectorBackendRef"` shows no `match` that handles `External`.
- Nothing in the repository calls a remote vector service; no driver code exists.
- `SecretString` serializes transparently (`shamir-types` `secret.rs`), so any `External` descriptor persisted through a library caller carries the key in clear in the `PersistedIndexes` blob. This is the card.

## Decision (binding)

The database does not talk to an external vector API; that is the client's business. Remove the dead variant, which removes the key from the persisted format entirely. Do not add secret references, scrubbing or any replacement.

## Binding behaviour

1. Delete `External` from `VectorBackendRef` (and the `SecretString` import if it becomes unused), the redaction test for it, and any doc comment that mentions an external backend. `VectorBackendRef` keeps `InProcessHnsw` as variant index 0, so the serialized form of every descriptor the engine can write today is unchanged byte for byte. Prove it with a test that encodes a `VectorConfig` with `InProcessHnsw` through the persistence codec used for `PersistedIndexes` and compares with a golden byte string pinned in the test.
2. A persisted blob that holds the removed variant (variant index 1) must fail to decode with the crate's ordinary typed codec error, not panic and not decode to a wrong variant. Add a test that builds such bytes from the golden form (replace the variant tag) and asserts the exact error kind; document this fail-closed behaviour in the type's doc comment and, if `docs/guide-docs/KNOWN_LIMITATIONS.md` has a section on persisted formats, one line there. State in the final report that no engine path could have written that variant.
3. Keep `VectorBackendRef` as an enum with one variant only if removing the enum would change the persisted bytes or widen the diff; say which you chose and why. Do not change `VectorConfig` fields or the serde attributes.
4. Public API: `VectorBackendRef::External` is removed from `shamir_index` and from the re-export in `shamir_engine::index2::kind` (it is the same type). List every workspace crate that referenced it (the expectation is none besides tests) and run `cargo check` on each dependent that compiles it.
5. Docs: `git grep -n -i "external" -- docs/guide-docs` for vector-backend promises; fix any doc that advertises an external vector backend. Do not touch the audit documents under `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review`.

## Tests (red first where the old code can fail)

The golden-bytes test and the fail-closed decode test above. The old code fails the second one only in the sense that variant 1 decodes; state that in the report. No behavioural engine test is needed.

## Scope

Crate `shamir-index` (`model/kind.rs`, `model/meta/persistence.rs` only if the test needs a codec entry point, the two test files) and docs. Coordination: P34-07b edits only `crates/shamir-index/src/vector/snapshot/*`; do not touch that folder.

## Decisions

RV-INDEX-CF86432E: **IMPLEMENT removal of the dead variant** (owner question answered by code evidence, 2026-11-01).

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

`cargo fmt -p <crate> -- --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Crates: shamir-index (check shamir-engine, shamir-db, shamir-server). Report real exit codes.

## Final report

Per item of the design stage: DONE / ALREADY-FIXED / SKIPPED(decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed ones you needed; anything that needs a build or test run; every deviation from the design with its written reason.
