# P1 stage H2 — p1-client-interner

## Goal

Client interner cache: dump retry, multi-repo map, epochs. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H2): A failed dump can be retried; multi-repo de-intern uses the right field map; sparse touch responses do not advance the epoch; unmapped names are reported; per-row allocations removed; ambient sync skipped when not needed.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-CLIENT-4BEB304C — `dump_repo`: failed dump poisons the OnceCell as "populated", later calls never re-dump, and the `Result` lies

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-client/src/interner_cache/ops.rs:210; crates/shamir-client/src/interner_cache/cache.rs:145; changed by: none
- R0 note (includes the fix site): OBSERVED: Transport and parse errors still resolve unit into get_or_init and dump_repo returns Ok; later calls skip the closure, including the documented unconditional re-dump. Fix sites: crates/shamir-client/src/interner_cache/ops.rs and cache.rs, fallible first-population state and subsequent dump behavior.

### RV-CLIENT-F2BD01C0 — Multi-repo v2 de-intern: first-match FieldMap can silently attach wrong field names

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-client/src/interner_cache/ops.rs:698; crates/shamir-client/src/interner_cache/ops.rs:788; changed by: none
- R0 note (includes the fix site): OBSERVED: aliases are discarded and every row accepts the first repository dictionary resolving its IDs; the new ref preflight does not exclude ordinary multi-repository batches. Fix site: interner_cache/ops.rs must carry result-to-repository provenance instead of first-success dictionary probing.

### RV-CLIENT-3594376F — Sparse touch responses advance the client's completeness epoch past missing IDs

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-client/src/interner_cache/ops.rs:288; crates/shamir-client/src/interner_cache/cache.rs:74; changed by: none
- R0 note (includes the fix site): OBSERVED: Sparse touch mappings still advance the completeness cursor through insert_entry and the response epoch; refresh subsequently requests only IDs beyond that cursor, excluding lower uncached IDs even with a compliant server. Fix sites: crates/shamir-client/src/interner_cache/cache.rs and ops.rs, separating sparse observations from contiguous completeness.

### RV-CLIENT-2B10814D — `touch_fields` silently omits names the server failed to map

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-client/src/interner_cache/ops.rs:287; crates/shamir-client/src/interner_cache/ops.rs:293; changed by: none
- R0 note (includes the fix site): OBSERVED: Touch parsing validates pair shapes but not requested-name completeness; an incomplete peer response still becomes a successful filter_map subset, although the ordinary server maps every name or errors. Fix site: crates/shamir-client/src/interner_cache/ops.rs, validation before returning touch_fields mappings.

### RV-CLIENT-1CF072D5 — v2 read path: per-row `ByteBuf` clone and per-row×repo `get_or_create` key allocations in de-intern

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-client/src/interner_cache/ops.rs:714; crates/shamir-client/src/interner_cache/cache.rs:184; changed by: none
- R0 note (includes the fix site): OBSERVED: Every IdBytes row is cloned before de-interning, and each candidate repository probe calls get_or_create, constructing owned database/repository strings. Fix site: crates/shamir-client/src/interner_cache/ops.rs, borrowed row processing and provenance-correct pre-resolved FieldMaps.

### RV-CLIENT-1C219006 — Ambient interner sync runs on every `execute` regardless of server version or cache state

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-client/src/client/api.rs:188; crates/shamir-client/src/interner_cache/ops.rs:395; changed by: none
- R0 note (includes the fix site): OBSERVED: Empty interner_epochs still triggers repository traversal and owned cache-key construction without a negotiated-version gate; smart pre-touch remains an explicit functional promise, not universally wasted work. Fix site: crates/shamir-client/src/client/api.rs for version-aware ambient-sync admission, preserving cold-v2 synchronization.

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
