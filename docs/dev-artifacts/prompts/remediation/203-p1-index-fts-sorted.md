# P1 stage F10 — p1-index-fts-sorted

## Goal

FTS stats/top-k, posting cache race, sorted batching, expr. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F10): `IndexExpr::Mod` cannot panic; FTS stats are consistent under empty/non-empty transitions, never underflow and never divide by zero; posting-cache fill cannot outlive an invalidation; sorted/unique apply batch their round-trips; FTS lookup is top-k bounded; `trusted_pure` is enforced at dispatch; no per-row deep clone of index definitions; the external API key is not stored in cleartext.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-INDEX-215567C1 — IndexExpr::Mod panics for i64::MIN modulo -1

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/model/query/expr.rs:147; crates/shamir-index/src/model/query/expr.rs:152; changed by: none
- R0 note (includes the fix site): OBSERVED: Public Mod evaluation guards only divisor zero before executing signed remainder; public Field evaluation can supply i64::MIN and divisor −1 remains accepted. INFERRED: This pair triggers Rust remainder overflow. Fix site: crates/shamir-index/src/model/query/expr.rs must handle the overflow pair through defined Result behavior.

### RV-INDEX-68011961 — `FtsRankedBackend::plan_update` emits unguarded BumpFtsStats on empty↔non-empty transitions — permanent doc_count/avg_doc_len drift

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/backends/fts_ranked_backend.rs:164; crates/shamir-index/src/backends/fts_ranked_backend.rs:223; changed by: none
- R0 note (includes the fix site): OBSERVED: Empty inserts contribute no count, but updates always emit both a negative old and positive new document adjustment, and apply_in_memory executes both unconditionally. INFERRED: Empty/nonempty transitions leave the document count unchanged when it should change. Fix site: crates/shamir-index/src/backends/fts_ranked_backend.rs must condition each stats adjustment on its corresponding nonempty document.

### RV-INDEX-D3ED367D — `lookup_by_index` posting-cache miss→scan→insert race can pin a stale entry past the writer's invalidation

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/base_index/index_manager/lookup.rs:441; crates/shamir-index/src/base_index/index_manager/lookup.rs:483; changed by: none
- R0 note (includes the fix site): OBSERVED: Miss publication still inserts an unversioned scan result, while writers only remove cached entries after writing; the reader gate protects DROP, not ordinary writes. INFERRED: A reader paused after its old scan can publish after writer invalidation. Fix site: crates/shamir-index/src/base_index/index_manager/lookup.rs, coordinating cache publication with writer invalidation.

### RV-INDEX-8F977BF5 — Per-row deep-clone of the whole index-definition set in every write planner

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/base_index/meta/index_info.rs:314; crates/shamir-index/src/base_index/sorted_index_manager/inner.rs:518; changed by: none
- R0 note (includes the fix site): OBSERVED: IndexInfo iteration clones each yielded definition, sorted iter_indexes clones the complete vector, and singular unique/sorted planners consume those APIs; batch snapshots only amortize other entry points. Fix sites: crates/shamir-index/src/base_index/meta/index_info.rs and crates/shamir-index/src/base_index/sorted_index_manager/inner.rs need borrowed snapshot access for singular planners.

### RV-INDEX-0577E3C4 — Sorted/unique apply paths issue one store round-trip per posting — the transact batching landed only for the regular-hash family

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/base_index/sorted_index_manager/plan.rs:251; crates/shamir-index/src/base_index/index_manager_unique/validate.rs:452; changed by: none
- R0 note (includes the fix site): OBSERVED: Direct sorted apply awaits each set/remove, and direct unique updates await separate removal and insertion; the sorted batch-create API and transactional batching do not replace these direct paths. Fix sites: crates/shamir-index/src/base_index/sorted_index_manager/plan.rs and crates/shamir-index/src/base_index/index_manager_unique/validate.rs need order-preserving batching with uniqueness coordination retained.

### RV-INDEX-DC5C642E — FTS lookup materializes every matching posting list and sorts all results — no top-k bound

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/backends/fts_ranked_backend.rs:309; crates/shamir-index/src/backends/backend.rs:68; changed by: none
- R0 note (includes the fix site): OBSERVED: FTS still collects complete per-token postings and fully sorts accumulated scores; IndexQuery::Fts has no result bound, and the backend trait changes concern vector promotion only. This is structural resource-cost debt, not measured latency. Fix site: crates/shamir-index/src/backends/fts_ranked_backend.rs, with an explicit bounded-query contract that preserves full-result/count callers.

### RV-INDEX-BCD7CAF0 — `trusted_pure` scalar gate is documented here but not enforced at this crate's dispatch boundary

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/model/query/expr.rs:178; crates/shamir-funclib/src/scalar_resolver.rs:118; changed by: none
- R0 note (includes the fix site): OBSERVED: Functional evaluation still calls unrestricted user-first dispatch, which validates arity but not is_indexable; engine DDL checks only admission, while direct construction and subsequent host replacement bypass that check. #297 makes replacement work but adds no dispatch trust gate. Fix site: crates/shamir-index/src/model/query/expr.rs needs trust-checked invocation of the same resolved entry through the resolver.

### RV-INDEX-CF7D545B — `FtsStats::on_delete` uses bare `fetch_sub` — underflow wraps to a huge `doc_count` with no saturating guard

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-index/src/fts/bm25.rs:87; crates/shamir-index/src/backends/fts_ranked_backend.rs:286; changed by: none
- R0 note (includes the fix site): OBSERVED: Public on_delete unconditionally fetch_subtracts both counters, and negative stats operations invoke it without a lower-bound guard. INFERRED: Invalid/double accounting wraps unsigned counters; the empty-update subtraction may instead be transiently canceled by its paired addition. Fix site: crates/shamir-index/src/fts/bm25.rs needs guarded deletion accounting, independently of correcting update transitions.

### RV-INDEX-AEEDB51B — `FtsStats`: torn (count, sum) reads and a divide-by-zero window in `avg_doc_len`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-index/src/fts/bm25.rs:72; crates/shamir-index/src/fts/bm25.rs:80; changed by: none
- R0 note (includes the fix site): OBSERVED: Count and total length remain separate relaxed atomics, with insertion incrementing count first and avg_doc_len sampling them independently; term_score divides by that average without a positive-value guard. INFERRED: A reader between first-document increments can observe average zero; this is floating-point scoring degradation, not an integer division panic. Fix site: crates/shamir-index/src/fts/bm25.rs needs a defined coherent or safely approximate stats-reading policy.

### RV-INDEX-CF86432E — External vector-backend API key persisted in cleartext inside the index-metadata blob

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-types/src/entries/secret.rs:54; crates/shamir-index/src/model/meta/persistence.rs:93; changed by: none
- R0 note (includes the fix site): OBSERVED: External credentials remain transparently serialized into the persisted descriptor envelope; the metadata change only bumps the legacy format version. This remains credential-carrier hardening, not a remote disclosure or a violation of the documented volume-encryption policy. Fix site: crates/shamir-index/src/model/meta/persistence.rs, replacing persisted credentials with secret references.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
