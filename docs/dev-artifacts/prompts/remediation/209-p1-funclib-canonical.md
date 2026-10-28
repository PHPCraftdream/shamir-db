# P1 stage H12 — p1-funclib-canonical

## Goal

Funclib canonical encode/hash contracts, error codes, docs. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H12): Canonical encode/hash contracts match their docs (non-finite floats, reserved fields, key order, versioned format); `object/pick` order as documented; one error-code catalog; no swallowed serialization failures; docs updated.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-FUNCLIB-0418C20F — encode's advertised lossless safe-variant roundtrip excludes neither non-finite F64 nor recognized typed map-key prefixes

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-types/src/types/value.rs:268; crates/shamir-funclib/src/functions/value/encode.rs:27; changed by: 7d4140db (#290)
- R0 note (includes the fix site): OBSERVED: default QueryValue decoding now preserves typed-looking keys verbatim, removing the prefix-loss facet; the unconditional F64 roundtrip promise remains, and locked serde_json 1.0.149 serializes non-finite F64 as null. Fix site: crates/shamir-funclib/src/functions/value/encode.rs must constrain/document the finite-number domain or adopt an explicit faithful conversion contract.

### RV-FUNCLIB-775ADABC — Generic canonical APIs do not exclude an interned _prev_hash field despite their unconditional reserved-field documentation

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/functions/value/canonical/core.rs:192; crates/shamir-funclib/src/functions/value/canonical/core.rs:198; changed by: none
- R0 note (includes the fix site): OBSERVED: The generic API still promises top-level exclusion, but detection compares only against the serialized string "_prev_hash"; interned numeric keys cannot match. Fix site: crates/shamir-funclib/src/functions/value/canonical/core.rs must narrow the contract or provide interner-aware exclusion.

### RV-FUNCLIB-E5A20918 — object/pick emits requested-key order rather than the documented source insertion order

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/functions/value/object.rs:13; crates/shamir-funclib/src/functions/value/object.rs:148; changed by: none
- R0 note (includes the fix site): OBSERVED: The module promises source insertion order, but pick iterates requested keys and inserts them in that order; source a,b requested as b,a is therefore reversed. Fix site: crates/shamir-funclib/src/functions/value/object.rs must align pick implementation and documented ordering.

### RV-FUNCLIB-54359750 — `canonical_hash` key ordering is msgpack-dependent but documented as name order; byte format has no version tag

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/functions/value/canonical/core.rs:184; crates/shamir-funclib/src/functions/value/canonical/core.rs:203; changed by: none
- R0 note (includes the fix site): OBSERVED: keys are still sorted by length-tagged MessagePack encodings despite the name-order claim, and canonical_bytes begins directly with value encoding without a format-version prefix. Migration damage remains prospective, not demonstrated. Fix site: crates/shamir-funclib/src/functions/value/canonical/core.rs needs accurate frozen-byte documentation and an explicit compatibility/version policy.

### RV-FUNCLIB-42F5C1C9 — Module docs advertise plain unqualified names; the wire protocol dispatches folder-qualified names

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/functions/value/encode.rs:3; crates/shamir-funclib/src/lib.rs:55; changed by: none
- R0 note (includes the fix site): OBSERVED: category documentation still advertises plain registration names while production register_builtins applies folder prefixes; lib.rs also still calls implemented categories stubs. Plain names remain valid for direct category registration only. Fix site: crates/shamir-funclib/src/functions/value/encode.rs and sibling category headers must distinguish local registration from production-qualified dispatch.

### RV-FUNCLIB-24C30E50 — `ScalarError` string-code taxonomy is unpoliced: no code catalog, drifting synonyms, zero context

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/registry.rs:20; crates/shamir-funclib/src/functions/text/validate/core.rs:363; changed by: none
- R0 note (includes the fix site): OBSERVED: ScalarError still accepts arbitrary owned string codes; the shared regex cache emits bad_regex while validate remaps the same failure to bad_pattern, preserving the unenforced taxonomy. Fix site: crates/shamir-funclib/src/registry.rs needs an explicit stable code catalogue and enforcement without silently renaming existing outputs.

### RV-FUNCLIB-AC384A3A — canonical `encode` re-serialises the reserved-key constant for every top-level map key

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/functions/value/canonical/core.rs:156; crates/shamir-funclib/src/functions/value/canonical/core.rs:193; changed by: none
- R0 note (includes the fix site): OBSERVED: Every top-level map key calls key_is_prev_hash, which freshly serializes the same reserved string; no cached encoding or hoisted comparison exists. Fix site: crates/shamir-funclib/src/functions/value/canonical/core.rs can reuse the reserved-key encoding without changing canonical bytes.

### RV-FUNCLIB-0CAAB4E1 — `canonical.rs` key serialization swallows failure (`unwrap_or_default`) on an integrity-critical path

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/functions/value/canonical/core.rs:187; crates/shamir-funclib/src/functions/value/canonical/core.rs:199; changed by: none
- R0 note (includes the fix site): OBSERVED: public generic canonicalization accepts arbitrary Serialize keys and replaces serialization errors with empty bytes, without a caller guard restricting keys to infallible strings. Fix site: crates/shamir-funclib/src/functions/value/canonical/core.rs must propagate key serialization failure or explicitly restrict its accepted key contract.

### RV-FUNCLIB-F3D98385 — Stale crate-level doc claims all non-math categories are "stubs"

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-funclib/src/lib.rs:12; crates/shamir-funclib/src/lib.rs:62; changed by: none
- R0 note (includes the fix site): OBSERVED: The crate header still calls remaining categories stubs despite registering implemented datetime, object, crypto and other categories. Fix site: crates/shamir-funclib/src/lib.rs needs the stale implementation-status sentence corrected.

## Binding rulings

These decisions are final; do not re-litigate them.

### Binding ruling RU9 — `canonical_hash` format (RV-FUNCLIB-54359750 and neighbours)

Decided by 2x rush reviewer + @oh (unanimous, 3/3): **option A — freeze the existing bytes as format v1 (unprefixed), fix the documentation, pin with literal known-answer vectors, reserve a version-marker range.**

Facts: `crates/shamir-funclib/src/functions/value/canonical/core.rs` serializes map keys with MessagePack (header included) and sorts on those bytes, so for `String` keys the order is UTF-8 length first, then bytes ("b" before "aa"); the docs claim name order. The live consumer is the registered scalar `crypto/canonical_hash`; its output reaches clients and the CAS protocol stores it in user records as `_prev_hash` (ordinary record data), so any byte change would invalidate every hash a client holds. No engine/WAL/replication code calls these functions; `shamir-connect`'s `audit_chain::canonical_bytes` is a separate function.

Acceptance:
1. For every input whose keys serialize today, the output of `encode`, `serialise_key` and `canonical_bytes` does not change by a single byte; all existing tests in `canonical/tests/canonical_tests.rs` pass unmodified.
2. New test file `canonical/tests/canonical_vectors_tests.rs` (registered in the manifest) with committed hex literals for `canonical_bytes` and `canonical_hash`, derived BY HAND from the spec, not by running the code under test (record the derivation in a comment). Cover: Null, Bool, negative Int, Str, Bin, List, Set (element order permuted), nested Map; F64 (+0.0 and -0.0 equal bytes, two NaN payloads equal bytes, an ordinary finite value); Dec and Big under the `T_STR` tag; a map with keys "b" and "aa" asserting "b" first; keys of length 31 and 32 (fixstr to str8 boundary) asserting the 31-byte key first; a top-level `_prev_hash` excluded and a nested `_prev_hash` kept.
3. Rewrite the docs in `core.rs` (module header and the key-ordering comments): keys are sorted by their MessagePack encoding (for `String` keys: UTF-8 byte length first, then bytes); the key bytes in the stream include the MessagePack header; for interned keys the order is by MessagePack uint encoding and the hash holds only within one interner; remove the "UTF-8 key name order" claim.
4. Version policy in the module docs, backed by a named constant or documented range: the current format is v1 with no prefix; leading bytes `0x0b..=0xff` are reserved for future version markers (a v1 stream always starts with a type tag in {00,01,02,03,06,07,08,09,0a}); any byte change must emit a marker and keep a v1 verifier. A test asserts that the first byte of `canonical_bytes` is in the type-tag set for every variant.
5. RV-FUNCLIB-0CAAB4E1: remove `unwrap_or_default` on key serialization; errors propagate: `canonical_bytes`/`canonical_hash` return `Result<_, CanonicalError>` (a `thiserror` type); the scalar maps the error to `ScalarError`; update the CAS test call sites; a test with a key type whose `Serialize` always fails asserts `Err`, not a hash. The vectors are unchanged.
6. RV-FUNCLIB-AC384A3A: compute the reserved-key encoding once; a test asserts the precomputed value equals `rmp_serde::to_vec(PREV_HASH_FIELD)`. Vectors unchanged.
7. RV-FUNCLIB-775ADABC: exclusion stays string-key only; document that `_prev_hash` is excluded only when the key serializes to the MessagePack string "_prev_hash"; do NOT add interner-aware exclusion to the generic path (a separate explicitly named API if ever needed). Vectors unchanged.
8. RV-FUNCLIB-0418C20F concerns `encode.rs` (the serde_json path), not `canonical_bytes`; its fix must not touch `canonical/core.rs`.

Compat: none (stored `_prev_hash` values, hashes held by clients and `SELECT canonical_hash(...)` results stay valid). In-crate API change only: the generic functions return `Result` (affects the scalar and `cas_sequenced_e2e.rs`).


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
