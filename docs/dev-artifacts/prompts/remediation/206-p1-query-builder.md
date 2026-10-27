# P1 stage H8 — p1-query-builder

## Goal

Query builder: macro paths, no panics, typed wire errors, secrets. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H8): Macros use only the crate's re-exported paths; `to_request_via_msgpack` returns an error; wire errors are crate types; `rows_as` decodes in one pass; passwords are a secret type; macro raw idents, dotted paths, trailing commas handled; imports at top.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-QUERY-BUILDER-C6841ED3 — `subscribe!` / `bind!` macros hardcode foreign crate paths, breaking the crate's dependency-hiding contract

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-builder/src/macros/mod.rs:74; crates/shamir-query-builder/src/macros/mod.rs:111; changed by: none
- R0 note (includes the fix site): OBSERVED: bind still names shamir_collections and subscribe still names shamir_query_types in consumer scope; the recent expansion-path repair affects procedural macros, not these expansions. INFERRED: Builder-only consumers still lack those foreign crate names. Fix site: crates/shamir-query-builder/src/macros/mod.rs, routing every foreign reference through builder-owned $crate paths.

### RV-QUERY-BUILDER-EE1751FB — `Batch::to_request_via_msgpack` panics on codec error -- public API, contradicts the crate's own stated ideology

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-builder/src/batch/batch_refs.rs:39; crates/shamir-query-builder/src/batch/batch.rs:96; changed by: none
- R0 note (includes the fix site): OBSERVED: The public helper retains both expects, accepts arbitrary QueryValue request IDs, and try_build does not validate ID depth; pinned rmp-serde still has a finite decoder depth budget. INFERRED: An over-deep ID reaches the decode expect; new validation also supplies additional panic paths. Fix site: crates/shamir-query-builder/src/batch/batch_refs.rs, returning build/encode/decode failures instead of panicking.

### RV-QUERY-BUILDER-6931DBDE — Public API leaks `rmp_serde` error types; decode errors are re-labeled as encode errors

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder/src/wire/mod.rs:33; crates/shamir-query-builder/src/wire/response/batch_response_ext.rs:35; changed by: none
- R0 note (includes the fix site): OBSERVED: ToWire still exposes rmp_serde encode errors and maps decode failures into encode::Error::Syntax; ResponseError publicly embeds decode::Error. Batch's new BuildError return does not change these APIs. Fix sites: crates/shamir-query-builder/src/wire/mod.rs and crates/shamir-query-builder/src/wire/response/batch_response_ext.rs, preserving codec phase behind builder-owned errors.

### RV-QUERY-BUILDER-028443E5 — `rows_as` deserializes via a per-record encode+decode round-trip instead of one batched pass

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder/src/wire/response/batch_response_ext.rs:90; crates/shamir-query-builder/src/wire/response/batch_response_ext.rs:186; changed by: none
- R0 note (includes the fix site): OBSERVED: rows_as still invokes deserialize_record separately for each row, allocating a MessagePack buffer and decoding each record's as_value projection. This establishes repeated structural cost, not measured slowdown. Fix site: crates/shamir-query-builder/src/wire/response/batch_response_ext.rs, reducing repeated codec setup while preserving projected record semantics.

### RV-QUERY-BUILDER-FC88A91A — Builder holds the plaintext password in a bare `String` until `build()`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder/src/ddl/policy/auth.rs:19; crates/shamir-types/src/entries/secret.rs:67; changed by: none
- R0 note (includes the fix site): OBSERVED: CreateUser still retains an ordinary password String until build converts it to SecretString; abandoning the builder bypasses the crypto-enabled wrapper's zeroizing Drop. Private fields and absent Debug limit the claim to memory hygiene. Fix site: crates/shamir-query-builder/src/ddl/policy/auth.rs, wrapping the password when the builder is constructed.

### RV-QUERY-BUILDER-MACROS-002C0148 — Raw identifiers are silently lowered to database names containing the Rust r# escape prefix

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder-macros/src/filter_lower.rs:332; crates/shamir-query-builder-macros/src/query_parse_lower.rs:22; changed by: none
- R0 note (includes the fix site): OBSERVED: Field segments and table identifiers still use Ident::to_string directly without unraw normalization; the changed lowering code repairs expansion paths, not identifier spelling. INFERRED: Raw spellings remain literal r#-prefixed database names. Fix sites: crates/shamir-query-builder-macros/src/filter_lower.rs and crates/shamir-query-builder-macros/src/query_parse_lower.rs, normalizing identifiers at database-name boundaries.

### RV-QUERY-BUILDER-MACROS-3C03A7DA — `group_by` / `order_by` accept only bare idents — no dotted paths, no string-literal field names

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder-macros/src/query_parse.rs:224; crates/shamir-query-builder-macros/src/query_parse.rs:266; changed by: none
- R0 note (includes the fix site): OBSERVED: group_by and order_by still parse bare Ident values, so dotted paths and string-literal field names remain loudly rejected; the recent parser repairs do not extend these lists. Fix site: crates/shamir-query-builder-macros/src/query_parse.rs, introducing path-capable list items and matching lowering rather than joining nested paths into a single segment.

### RV-QUERY-BUILDER-MACROS-03BA722D — Trailing comma in `group_by`/`select`/`order_by` lists is rejected with a confusing error; `peek_clause_keyword_after_comma` contains dead branches

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder-macros/src/query_parse.rs:253; crates/shamir-query-builder-macros/src/query_parse.rs:640; changed by: none
- R0 note (includes the fix site): OBSERVED: EOF lookahead still stops list consumption with the terminal comma unconsumed, leading to the generic clause-order error at line 301. The asc/desc branches remain reachable, not dead. Fix site: crates/shamir-query-builder-macros/src/query_parse.rs, explicitly consuming permitted terminal commas or reporting a targeted diagnostic.

### RV-QUERY-BUILDER-MACROS-EEE42189 — Quadratic token re-interpolation when accumulating builder chains in loops

- R0 verdict: `unverified`, risk `low`; evidence: crates/shamir-query-builder-macros/src/query_parse_lower.rs:79; crates/shamir-query-builder-macros/src/query_parse_lower.rs:120; changed by: none
- R0 note (includes the fix site): OBSERVED: Both loops still re-interpolate the growing chain; #304 and #306 do not remove that pattern. Missing premise: copying/concatenation semantics of the pinned Rust 1.95.0 compiler TokenStream backend, whose source is unavailable locally. Fix site: query_parse_lower.rs, accumulate fragments before one final interpolation.

### RV-QUERY-BUILDER-MACROS-39AA68B5 — Mid-function `use syn::BinOp;` violates "Imports at the top"

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-query-builder-macros/src/filter_lower.rs:55; changed by: none
- R0 note (includes the fix site): OBSERVED: lower_binary still contains the function-local use syn::BinOp with no documented exception; the diagnostics-only change leaves this convention-only debt intact. Fix site: crates/shamir-query-builder-macros/src/filter_lower.rs, hoisting BinOp into the file-header imports.

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
