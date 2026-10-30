# P34-07 - shamir-index: n-gram budget, vector snapshot, backend secrets, doc/import cleanup

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Bound n-gram tokenization with a streaming visitor and complete legacy cleanup, handle legacy backend credentials per the owner answer, fix the stale lifecycle documentation and local imports, and (bench-gated) stream the vector snapshot while keeping the repaired generation/prune ordering.

This is one change set for shamir-index: 4 active findings and 3 P2 carry-overs, effort units 19 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-index`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- The n-gram visitor never builds a per-value Vec of all grams and old-value cleanup is complete without truncation (0695AADA).
- CF86432E is skipped until the owner answers.
- 4DC29EFB: before/after memory numbers from CI; the repaired generation/prune ordering is preserved.
- 40BCFF22 and F39434C2 follow their P2 recipes; the lifecycle documentation matches the shipped IndexState enum (20415F00); remaining local imports are hoisted (32C98E9F).

## Coordination with other tasks

Wave 2; starts after P34-03 are merged. CCF2090E (P34-03) reads index meta_envelope.rs and pins its bytes; index edits come after the pin exists.
- CCF2090E in P34-03 pins the buffer-config bytes through meta_envelope.rs; do not change that envelope here.
- tokenizer.rs and persistence.rs are shared with P34-12, which starts after this stage.

## Decisions

Recorded by the orchestrator from the design decider (@oh) on 2026-10-30. Cards: RV-INDEX-0695AADA, RV-INDEX-CF86432E. The full decision paragraph sits in the finding below.

- L3-RV-INDEX-0695AADA: **IMPLEMENT** (change as decided).
- L3-RV-INDEX-CF86432E: **ESCALATE** (waits for the owner).

## Findings (ordered by risk, then size)

### L3-RV-INDEX-0695AADA - `NgramTokenizer` output is unbounded — indexing-time memory/write amplification from one long token

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Continuing the P2 decision for RV-INDEX-0695AADA. Add a crate-private streaming visitor in src/fts/ngram_budget.rs (re-exported or wrapped on TokenizerEnum as pub(crate) for_each_token_hash). For the n-gram tokenizer it lowercases each word once and calls token_hash on borrowed window slices of the lowered word, with no per-gram String. Other tokenizer variants keep iterating tokenize(). Switch only the unbudgeted cleanup paths: FtsBackend::tokenize_record (backends/fts_backend.rs:70-81, used by plan_delete and the old side of plan_update) and FtsRankedBackend::tokenize_with_freq (fts_ranked_backend.rs:136-150, where doc_len = number of visited tokens), including the stats rebuild that uses it. Admission paths stay on try_tokenize, unchanged. Never truncate or skip: cleanup must still remove every posting. Must stay identical: hash set, term frequencies and doc_len for every input, the persisted bytes, the existing golden vectors, the budget constants and errors. Add doc comments on Tokenizer::tokenize and TokenizerEnum::tokenize (fts/tokenizer.rs:21-23,49-57) saying output is unbounded, is for cleanup of already-admitted values only, and that new values and queries must use try_tokenize. Regression tests (rv_index_0695aada_*): over-budget (n=255, a ~70k-char word) and within-budget inputs, including non-ASCII, short words and n=1. Streamed hashes, frequencies and doc_len must equal the tokenize()+token_hash results, and plan_delete on an over-budget legacy record still returns a RemovePosting for every distinct gram. Out of scope: making the public trait fallible, changing budgets, budgeting cleanup.
- Evidence at `07bc804c`: crates/shamir-index/src/fts/ngram_budget.rs:54-72; crates/shamir-index/src/fts/tokenizer.rs:127-144; crates/shamir-index/src/backends/fts_backend.rs:172-173,197-205
- Recipe: Stream legacy cleanup without truncation and define a bounded/fallible public tokenizer contract.
- Verifier note: New values are budgeted, but public infallible tokenization and old-value cleanup remain unbounded; preserve complete cleanup.

### L3-RV-INDEX-CF86432E - External vector-backend API key persisted in cleartext inside the index-metadata blob

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: ESCALATE** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). The owner must decide what happens to legacy inline External vector credentials. Today they are grandfathered and rewritten in cleartext on every index-metadata save (model/meta/persistence.rs:93-115,123-134). That was a deliberate P1 decision, with a test that the legacy descriptor still saves; the api_key_ref slot in options is reserved for later. The credential has no consumer (build_backend.rs:57-59 falls back to HNSW). Options: (A) recommended: scrub on decode in load_index2_metadata and before encode. Replace a non-empty api_key_secret with an empty SecretString so the registry and the blob agree; emit one warn per index (id/name only, never the secret); keep rejecting new credentials; update the P1 grandfather test to expect scrubbing; add a KNOWN_LIMITATIONS note. No format change, old binaries read it, but the scrub is irreversible, and earlier bytes in storage history are only dropped by compaction. (B) Keep grandfathering until secret references (api_key_ref) and an External backend exist, then migrate. Accepted risk, no change now. (C) Build runtime secret references now: not recommended, there is no consumer.
- This card waits for the owner: do not implement it; report it as SKIPPED(owner).
- Evidence at `07bc804c`: crates/shamir-index/src/model/meta/persistence.rs:93-115,123-134
- Recipe: Introduce runtime secret references and explicitly migrate or remove grandfathered inline credentials.
- Verifier note: New credentials are rejected, but matching legacy cleartext is rewritten; secret provisioning and migration need an owner.

### L4-RV-INDEX-32C98E9F - `use` statements inside function/block bodies across six production files

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-index/src/model/query/expr.rs:13-22; crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:56,253; crates/shamir-index/src/vector/hnsw_adapter/fit.rs:92
- Recipe: Hoist remaining ordinary HNSW Entry variant imports to file headers without changing names or cfg scope.
- Verifier note: Some imports moved, but ordinary production-local Entry imports remain in two HNSW files.

### L4-RV-INDEX-20415F00 - Stale lifecycle doc contradicts the shipped `IndexState` wire enum; minor `IndexRecordKey` API warts

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/-`.
- Evidence at `07bc804c`: crates/shamir-index/src/model/runtime/lifecycle.rs:11-14,31-56; crates/shamir-index/src/model/runtime/state.rs:49-73
- Verifier note: First pass missed the remaining explicit two-variant enum claim in the lifecycle introduction despite its corrected later section.

## Carried over from the P2 sprint

These P2 cards stayed open after the sprint (state from `P2-SPRINT-REPORT-2026-10-30.md`). Their evidence and recipe are the P2 revalidation at master `7f04e096`; re-locate the code by symbol.

### RV-INDEX-4DC29EFB - Vector snapshot dump/load fully materializes the graph and sidecar in RAM

- Sprint state: DEFERRED: streaming snapshot (owner decision in P2).
- Risk `medium`; fix size/kind `L/code`.
- Evidence: crates/shamir-index/src/vector/snapshot/codec.rs:368-369,440-441; crates/shamir-index/src/vector/snapshot/load.rs:120-124,216-220
- Recipe: Stream bounded snapshot chunks and move sidecar ownership while preserving generation coherence.
- Verifier note: Whole-index intermediates remain on dump and restore; bounded streaming must preserve generation publication coherence.

### RV-INDEX-40BCFF22 - Vector snapshot v1 back-compat is claimed but has no working decode path, and the only test is vacuous

- Sprint state: PARTIAL: v1 layouts decode only with a supported hnsw_rs graph version; a 0.3.4 graph is rejected and the index rebuilds from source.
- Risk `medium`; fix size/kind `M/code`.
- Evidence: crates/shamir-index/src/vector/snapshot/load.rs:46,94; crates/shamir-index/src/vector/tests/quant/quantization_snapshot_tests.rs:385-396
- Recipe: Add historical v1 disk mirrors and version dispatch, tested against genuine historical bytes.
- Verifier note: Historical layouts differ; direct current-type decoding and relabelled current fixtures do not implement promised v1 compatibility.

### RV-INDEX-F39434C2 - Silent degradation fallbacks with no logging

- Sprint state: PARTIAL: logging without a capture test.
- Risk `low`; fix size/kind `M/code`.
- Evidence: crates/shamir-index/src/fts/bm25.rs:105-127; crates/shamir-index/src/backends/fts_ranked_backend.rs:184-185,458-460; crates/shamir-index/src/base_index/sorted_index_manager/covering.rs:91
- Recipe: Make remaining decode skips, defaults, encoding fallbacks and rejected arithmetic observable or explicit errors.
- Verifier note: Checked arithmetic replaces wrapping, but rejected updates, corrupt posting defaults and rebuild skips remain silent.

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

`cargo fmt -p shamir-index -- --check`; `cargo clippy -p shamir-index --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
