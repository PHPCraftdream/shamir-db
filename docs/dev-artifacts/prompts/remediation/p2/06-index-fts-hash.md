# P2-06 - shamir-index (FTS, backends, hash identity): persisted hash identity, IndexError, FTS degradation

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Keep structured errors through IndexError, make FTS degradation visible, bound the n-gram tokenizer with fixed budgets, and pin the current persisted hash identities with golden vectors, doc comments and a KNOWN_LIMITATIONS entry. The persisted-identity redesign itself is deferred as accepted risk (follow-up index identity v2).

This is one change set for shamir-index: 9 findings, effort units 28 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. Files under src/fts/**, src/backends/**, base_index/infra and index_manager_unique. Hash changes need a persisted-index migration/rebuild decision.

## Scope

Crates: `shamir-index`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- Golden-vector tests pin the current persisted hash outputs for FTS tokens, regular/unique keys and functional postings (they pass on the old code by design and fail if any hashing changes); doc comments and a KNOWN_LIMITATIONS entry record the accepted risk. No persisted byte changes.
- IndexError keeps the structured DbError identity across boundaries (decided: additive boxed source-bearing variant).
- Corrupt FTS posting values and silent fallbacks are logged and surfaced; the n-gram tokenizer is bounded by fixed per-value budgets (1,048,576 grams / 16 MiB) with a typed TokenBudgetExceeded error, never truncation.

## Coordination with other tasks

Suggested wave 2; no strict predecessor.
- Owns src/fts/**, src/backends/**, base_index/infra and index_manager_unique. tokenizer.rs is also edited by P2-07 (32C98E9F): P2-07 starts after P2-06. New file src/fts/ngram_budget.rs keeps the tokenizer.rs diff small.

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: 2BA3B945, 53CEF405, 9E472725, 0695AADA, 605EEC5D, FBB72544.

- RV-INDEX-2BA3B945: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Preserve existing string variants and add a boxed source-bearing DbError variant with centralized conversions. Carry actual storage failures through it, propagate metadata Store errors directly, and recover the original DbError at engine boundaries. Include engine adapters in scope; preserve wire encoding and existing constructors, while updating exhaustive matches for the additive enum variant. CI must prove an injected Io error retains its variant, ErrorKind, and source through index-to-engine propagation.
- RV-INDEX-53CEF405: **DEFER** (design authority @oh (read-only agent), accepted by orchestrator, 2026-10-29). DEFER (@oh, accepted risk): the identity redesign is not done in this sprint; see RV-INDEX-605EEC5D for the golden vectors, doc comments and KNOWN_LIMITATIONS entry that P2-06 adds. Do not change any hashing or persisted byte. Realistic exploitability: a writer can already add any token to their own document; postings live in an ordered KV store keyed by the u64, so classic hash flooding does not apply. Follow-up design task 'index identity v2' (verify canonical value on every unique hit; framed canonical bytes as authority; bump LEGACY_INDEX_FORMAT_VERSION 3->4; per-table index2 identity marker reusing the Building self-heal; update the golden vectors in the same change).
- RV-INDEX-9E472725: **DEFER** (design authority @oh (read-only agent), accepted by orchestrator, 2026-10-29). DEFER (@oh, accepted risk, strongest of the four): a constant-work collision family exists for composite Int unique indexes, so a writer can pre-empt a predictable tuple and cause a false DuplicateKey. Not changed in this sprint (persisted-format change; see RV-INDEX-605EEC5D for what P2-06 adds: golden vectors, doc comments, KNOWN_LIMITATIONS entry). Follow-up 'index identity v2' task: verify the canonical value on every unique hit first (no format change, can land first), then framed canonical bytes with a LEGACY_INDEX_FORMAT_VERSION bump.
- RV-INDEX-0695AADA: **IMPLEMENT** (design authority @oh (read-only agent), accepted by orchestrator, 2026-10-29). IMPLEMENT-MODIFIED (@oh). Fixed constants, not configuration (a per-index setting would change the persisted `IndexKind`; a deployment-wide one needs plumbing outside shamir-index). The budget applies to `NgramTokenizer` only. Add a new file `src/fts/ngram_budget.rs` (keeps the `tokenizer.rs` diff small for P2-07) with crate constants `MAX_NGRAM_TOKENS_PER_VALUE = 1_048_576` and `MAX_NGRAM_BYTES_PER_VALUE = 16 * 1024 * 1024` (= one data frame), counted cumulatively over ONE `tokenize` call (one field value). Add `TokenizerEnum::try_tokenize -> Result<Vec<Cow<str>>, IndexError>` (non-n-gram variants always `Ok`, output identical) and a new additive variant `IndexError::TokenBudgetExceeded { limit_kind: &'static str, limit: u64, observed: u64 }` (no external crate matches `IndexError` variants; the engine maps it via `table_manager_crud.rs:98`). The check runs per word BEFORE that word's grams are emitted: gram count = chars - n + 1, gram bytes computed exactly in a non-allocating pass; replace the `boundaries: Vec<usize>` with lagging `char_indices` iterators. On excess return the error: never truncate, never skip. Budgeted paths: `plan_insert`, the NEW side of `plan_update`, and query tokenization (`tokenize_query` and its `fts_backend.rs` twin) in both `FtsBackend` and `FtsRankedBackend`. UNBUDGETED (must keep calling the infallible `tokenize`): the OLD side of `plan_update`, `plan_delete`, and the stats `rebuild` (`fts_ranked_backend.rs:381-397`), so a record admitted before this change can always be updated and deleted. Confirm (report, do not change engine) that a budget error during index2 backfill goes through the existing failed-self-heal path (`shamir-engine/.../write_domain_barrier.rs:285`) rather than failing table open. Must stay compatible: within-budget output byte-identical to today (same grams, same order, same `token_hash`), no persisted format change. Tests (`rv_index_0695aada_*`): over-budget bytes (n=255, ~70k-char run) and over-budget count (n=2, >1 Mi chars) -> `plan_insert` returns `TokenBudgetExceeded` (old code returns `Ok`); `plan_delete` and old-side `plan_update` on the same record still succeed; an over-budget query -> typed `lookup` error; a golden comparison showing within-budget output equals the pre-change tokenizer output.
- RV-INDEX-605EEC5D: **IMPLEMENT** (design authority @oh (read-only agent), accepted by orchestrator, 2026-10-29). SPLIT (@oh): in P2-06 change NO persisted byte. (a) Add golden-vector tests (`rv_index_605eec5d_golden_*`, `#[cfg(target_pointer_width = "64")]`) that pin today's exact outputs through existing visibility (no widening): `token_hash` for fixed ASCII/non-ASCII/empty tokens and the FTS posting key from `FtsRankedBackend::plan_insert`; `build_index_key_from_record` (and the lookup-side key from inside `base_index` tests) for Int, Str, F64-integral, Dec, Big and a composite (Int, Int) tuple; the functional posting keys from `FunctionalBackend::plan_insert` for scalars, List, Set and ONE Map in a fixed insertion order, with a comment naming FBB72544 as a known, deferred order dependence. These tests pass on the old code by design (a guard, not a behavior fix); they must fail if `rustc-hash` output or any hashing function changes. (b) Add doc comments on `token_hash`, `compute_leaf_hashes` / `compute_lookup_hashes` and functional `hash_inner` saying they are persisted identities, may change only with an identity-version bump, and are guarded by the golden tests. Add a `KNOWN_LIMITATIONS.md` section 3 entry: hash-only identity for unique and FTS postings (crafted collisions possible for writers), functional Map hashing depends on insertion order, remedy for affected indexes = DROP + CREATE. Do not touch `Cargo.toml` (no exact `rustc-hash` pin). Report 605EEC5D as PARTIAL (golden keys done; portable identity deferred).
- RV-INDEX-FBB72544: **DEFER** (design authority @oh (read-only agent), accepted by orchestrator, 2026-10-29). DEFER (@oh): index2 (FTS and functional) has no identity-version gate, so changing the Map hash now would silently break lookups on existing data and force two rebuilds. Fix together with the 'index identity v2' follow-up. P2-06 only pins today's bytes with golden vectors (see RV-INDEX-605EEC5D) and names this card in a comment.

## Findings (ordered by risk, then size)

### RV-INDEX-2BA3B945 - `IndexError` is stringly-typed; structured `DbError`s are flattened at every boundary

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/backends/backend.rs:100-109; crates/shamir-index/src/model/meta/persistence.rs:132-135
- Recipe: Preserve typed storage errors and sources through index and engine boundaries.
- Note: Named boundaries still flatten errors; not every crate API does.
- Second pass: Reachable storage failures still lose variants and sources; typed public error propagation crosses crate contracts.

### RV-INDEX-53CEF405 - FTS posting keys hash untrusted token text with unkeyed `FxHasher` (`token_hash`)

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/fts/tokenizer.rs:447-452; crates/shamir-index/src/backends/fts_ranked_backend.rs:136-158
- Recipe: Choose collision-resolving token identity with an explicit persisted FTS rebuild/version policy.
- Note: Unkeyed u64 identity remains; tokenizer-valid chosen collision not demonstrated.
- Second pass: Document and query paths use hash-only token identity; tokenizer-valid chosen-collision exploitation is not demonstrated.

### RV-INDEX-9E472725 - Regular + unique index keys use two correlated FxHasher streams as "collision resistance"; unique constraints are enforced on hash alone

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/base_index/infra/index_keys.rs:227-258; crates/shamir-index/src/base_index/index_manager_unique/validate.rs:310-345; Cargo.lock:3030
- Recipe: Choose collision-resolving canonical-value identity and an index migration/rebuild policy.
- Note: Canonical numeric equality fixes did not remove correlated hash-only identity.
- Second pass: Correlated dual hashing remains the sole value identity; unique conflicts inspect RecordId rather than canonical values.

### RV-INDEX-872016D9 - `FtsRankedBackend::plan_update` tokenizes the old record twice

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/backends/fts_ranked_backend.rs:152-154,249-251
- Recipe: Tokenize the old record once and derive both its token set and document length.
- Note: Duplicate old-record tokenization remains directly visible.
- Second pass: Update directly tokenizes the old record twice; one frequency result can supply both membership and length.

### RV-INDEX-0695AADA - `NgramTokenizer` output is unbounded — indexing-time memory/write amplification from one long token

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/fts/tokenizer.rs:143-158; crates/shamir-index/src/backends/fts_ranked_backend.rs:140-145
- Recipe: Define explicit token/work budgets and propagate typed over-budget errors without silent truncation.
- Note: N-gram allocation precedes frequency deduplication and lacks an index-local budget.

### RV-INDEX-605EEC5D - Persisted posting keys depend on FxHasher output stability, with no version coupling and a caret-pinned dependency

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/Cargo.toml:36; crates/shamir-index/src/fts/tokenizer.rs:447-452; crates/shamir-index/src/model/meta/persistence.rs:37
- Recipe: Define portable persisted-hash identity, golden keys and explicit upgrade or rebuild rules.
- Note: Format version 3 still does not couple dependency algorithm or target identity.

### RV-INDEX-9733F1F3 - Corrupt FTS posting values are silently replaced with `tf=1, doc_len=1`

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/backends/fts_ranked_backend.rs:181-187
- Recipe: Keep empty legacy defaults but reject corrupt nonempty postings, with ranked-lookup regression coverage.
- Note: Nonempty decoding failures still silently fabricate posting statistics.

### RV-INDEX-F39434C2 - Silent degradation fallbacks with no logging

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/fts/bm25.rs:105-127; crates/shamir-index/src/backends/fts_ranked_backend.rs:184-185,458-460; crates/shamir-index/src/base_index/sorted_index_manager/covering.rs:91
- Recipe: Make remaining decode skips, defaults, encoding fallbacks and rejected arithmetic observable or explicit errors.
- Note: Wrapping arithmetic is removed; several silent degradation paths remain.
- Second pass: Checked arithmetic replaces wrapping, but rejected updates, corrupt posting defaults and rebuild skips remain silent.

### RV-INDEX-FBB72544 - FunctionalBackend Map hashing is insertion-order dependent (byte-identity floor violation)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/backends/functional_backend.rs:180-187
- Recipe: Canonicalize recursive Map hashing and provide persisted-index rebuild handling and order-parity tests.
- Note: Numeric and Set fixes did not change insertion-ordered Map hashing.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- Cards marked `OWNER DECISION` are implemented only if the "Decisions" section below contains the answer; otherwise skip them and say so in the final report.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p shamir-index -- --check`; `cargo clippy -p shamir-index --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
