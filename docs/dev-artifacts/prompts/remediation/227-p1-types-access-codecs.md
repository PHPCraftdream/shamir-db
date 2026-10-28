# P1 stage H14 — p1-types-access-codecs

## Goal

Types: fail-closed access meta, codec errors, docs, layout. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H14): `ResourceMeta::from_record` fails closed; `is_system()` exact; one thiserror `CodecError`; codecs README matches code; `access.rs` split; public surface tested; nits applied.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-TYPES-15ECB3A6 — Fail-open mode fallback in `ResourceMeta::from_record`

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-types/src/entries/access.rs:275; crates/shamir-db/src/shamir_db/shamir_db/access/meta.rs:55; changed by: none
- R0 note (includes the fix site): OBSERVED: malformed-present mode values still fail numeric conversion into Mode::OPEN, and live catalogue metadata resolution calls this decoder without validating the field. Propagated catalogue-read errors do not guard malformed successful reads; ancestor and owner-only Manage checks remain separate constraints. Fix site: crates/shamir-types/src/entries/access.rs must distinguish malformed-present mode from absent legacy mode.

### RV-TYPES-F8FA8E1F — `RecordId::is_system()` misclassifies real records created within ~71 minutes after CUSTOM_EPOCH; the comment asserts this cannot happen

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-types/src/types/ids/record_id.rs:45; crates/shamir-types/src/types/ids/record_id.rs:108; changed by: none
- R0 note (includes the fix site): OBSERVED: Constructors store signed big-endian relative microseconds, but classification checks only four zero bytes; INFERRED: relative timestamps from 0 through 2^32−1 therefore classify as system records, with no production classification caller found. Fix site: reconcile the encoding, discriminator and impossible-zero claim in crates/shamir-types/src/types/ids/record_id.rs.

### RV-TYPES-83113A25 — Two public, rival `CodecError` enums; the basic/bincode one is hand-rolled, violating the documented thiserror rule

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-types/src/codecs/basic/bincode.rs:8; crates/shamir-types/src/codecs/error.rs:4; changed by: none
- R0 note (includes the fix site): OBSERVED: public bincode helpers still return a manually implemented Serialize/Deserialize CodecError separate from the adjacent thiserror Encode/Decode enum. Wrong imports produce static type errors, not demonstrated silent runtime matching failures. Fix site: crates/shamir-types/src/codecs/basic/bincode.rs must reconcile its public error type with the library convention and existing callers.

### RV-TYPES-12EEFFA6 — Nit: duplicated public `CodecError` name, malformed doctests, dead error variant, misnamed error variant

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-types/src/codecs/basic/bincode.rs:26; crates/shamir-types/src/record_view/lens/cursor.rs:139; changed by: none
- R0 note (includes the fix site): OBSERVED: the separate public CodecError, stale unfenced examples, and NonBinKey response to invalid string markers remain; source search finds PathNotFound construction only in a test. Cursor changes add traversal accounting, not these corrections. Fix sites: crates/shamir-types/src/codecs/basic/bincode.rs, crates/shamir-types/src/types/value_error.rs, and crates/shamir-types/src/record_view/lens/cursor.rs retain the named hygiene issues.

### RV-TYPES-437423C0 — src/codecs/README.md documents APIs that no longer exist (and wrong semantics)

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-types/src/codecs/README.md:347; crates/shamir-types/src/codecs/README.md:288; changed by: 803f57a0 (#307)
- R0 note (includes the fix site): OBSERVED: the new projection section and amended round-trip claims document Dec/Big-to-Str and Set-to-List correctly, closing that portion. Removed InternedCodec/CodecFormat surfaces and the false infallible, panicking deintern_key description remain advertised. Fix site: crates/shamir-types/src/codecs/README.md must reconcile its remaining API examples and conversion semantics with current exports.

### RV-TYPES-88540F60 — `access.rs` bundles identity, mode-bits, policy and error types into one 716-line file -- one-file-one-export violation

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-types/src/entries/access.rs:197; crates/shamir-types/src/entries/access.rs:371; changed by: none
- R0 note (includes the fix site): OBSERVED: Resource metadata and resource addressing still share the identity/mode/policy implementation file; this is maintainability debt, not a runtime defect. Fix site: split the independent exports in crates/shamir-types/src/entries/access.rs.

### RV-TYPES-65DDE5B8 — Untested public surface with documented contracts: `Interner::generation`, `ResourceMeta::owner_field`, `principal64_from_username`, `SecretString::into_inner`, `ResourcePath::WasmCompiler`

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-db/src/shamir_db/tests/access/enforcement_tests.rs:714; crates/shamir-types/src/entries/tests/secret_tests.rs:1; changed by: none
- R0 note (includes the fix site): OBSERVED: owner-absence versus explicit-System assertions and compiler-permission denial coverage remain meaningful; the changed enforcement file only adapts create_db error handling. Types source search still finds no direct generation call or SecretString::into_inner test. Fix sites: crates/shamir-types/src/core/interner/tests and crates/shamir-types/src/entries/tests/secret_tests.rs need the remaining discriminating contract oracles.

### RV-TYPES-F8CF2706 — API-polish bundle (five items; two folded into other findings)

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-types/src/types/value.rs:19; crates/shamir-types/src/macros/mpack.rs:286; changed by: none
- R0 note (includes the fix site): OBSERVED: UserValue remains the same type as production QueryValue despite tests-only replacement guidance, and the advertised sealed trait has no sealing bound; random Default and signed pre-epoch encoding remain counter-evidence to the rejected subclaims. Fix sites: correct alias guidance in crates/shamir-types/src/types/value.rs and the sealing claim or implementation in crates/shamir-types/src/macros/mpack.rs.

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
