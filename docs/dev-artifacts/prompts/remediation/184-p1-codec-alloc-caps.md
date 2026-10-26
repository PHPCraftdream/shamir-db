# P1 stage G9 — p1-codec-alloc-caps

## Goal

Decode-time allocation caps and u64 decode split. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G9): Every decode-time allocation is capped by the remaining input; u64 > i64::MAX decodes as Big in every decoder and as the v1 string projection in FilterValue (ruling RU7); the recursion-limit premise is verified.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-CLIENT-98656AAA — Opaque IdBytes map counts trigger uncapped allocation before body validation

- R0 verdict: `partially-fixed`, risk `high`; evidence: crates/shamir-types/src/record_view/lens/view.rs:341; crates/shamir-types/src/codecs/interned/codec.rs:227; changed by: c88caa80 (#299)
- R0 note (includes the fix site): OBSERVED: public index preallocation is now bounded by body bytes, but client de-interning still reserves view.len() from a header-only RecordView before traversal; opaque IdBytes still reach that consumer. Fix site: codecs/interned/codec.rs must bound or validate declared cardinality before record_view_deintern_with allocates.

### RV-SDK-E55BCA90 — SDK Value trusts MessagePack collection counts for eager allocation

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-sdk/src/api/value.rs:122; crates/shamir-sdk/src/api/value.rs:130; changed by: none
- R0 note (includes the fix site): OBSERVED: Both visitors still pass size_hint directly to Vec::with_capacity before reading elements; the locally inspected locked rmp-serde 1.3.1 implementation returns the declared remaining u32 count. INFERRED: a tiny truncated Array32/Map32 payload can request enormous allocation before decoding fails. Fix site: crates/shamir-sdk/src/api/value.rs must bound eager collection preallocation.

### RV-TYPES-18AC0CFC — Split-brain decode contract for msgpack u64 &gt; i64::MAX (Big vs Str depending on decoder)

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-types/src/types/value.rs:189; crates/shamir-types/src/codecs/interned/messagepack.rs:183; changed by: none
- R0 note (includes the fix site): OBSERVED: the generic visitor still promotes oversized MessagePack U64 to Big, whereas the custom decoder returns decimal Str; From<usize> also retains its direct i64 cast. The new intentional Big-to-string encoding projection does not reconcile decoding identical U64 bytes. Fix site: crates/shamir-types/src/codecs/interned/messagepack.rs must align the oversized-U64 decoding contract with the generic visitor.

### RV-SDK-07A35F13 — Default rmp-serde recursion limit prevents guest-stack blowup

- R0 verdict: `unverified`, risk `—`; evidence: crates/shamir-sdk/src/api/value.rs:121; Cargo.lock:2938; changed by: none
- R0 note (includes the fix site): OBSERVED: Recursive Value decoding still delegates to rmp-serde 1.3.1; its locally inspected array/map dispatch decrements a default 1024-depth counter. Missing premise: guest-target stack headroom and per-level stack consumption before that counter rejects input. Fix site: api/value.rs and its guest decoding entry points, establish an explicit stack-safe nesting bound.

## Binding rulings

These decisions are final; do not re-litigate them.

### Binding ruling RU7 — one decode rule for msgpack u64 > i64::MAX (RV-TYPES-18AC0CFC, RV-QUERY-TYPES-49564B25)

Decided by 2x rush reviewer + @oh (unanimous, 3/3).

RULING: every decoder turns an unsigned integer above `i64::MAX` into the exact `Value::Big(BigInt::from(u))` (serde `ValueVisitor`, the interned zero-copy and rmpv decoders, and the `RecordView` lens through a new non-allocating Big variant that materializes as `InnerValue::Big`). `From<u64>` and `From<usize>` give the same infallible `Big` (no `TryFrom`). `FilterValue` has no Big variant, so it carries the value as its D2 v1 projection `FilterValue::String(u.to_string())`, exactly like `lit_u64` and the BatchOp path; it must never become `Int`, `Float` or an error. Consistent with P0 D1-S2/D2: `Big` encodes as a decimal `str` on the v1 wire, the encoder is untouched, numeric equality stays on `NumericKey`.

Facts: serde side already promotes (`crates/shamir-types/src/types/value.rs` visit_u64, legacy `u:` and `From<u64>`). Differing sites: `codecs/interned/messagepack.rs` (`Marker::U64` arms in `decode_value` and the rmpv/legacy decoder: decimal `Str`), `record_view/lens/read_value.rs` (owned `Str`), `From<usize>` (`as i64` wraps). In production the lens already behaves as Big because `scalar_at` returns `None` for the owned edge and the filter falls back to `materialize_at`; only `Kind` differs (lens `Scalar`, tree `NonComparable`). Only the direct `CreateCursor` `ReadQuery` decode falls through to `Float(f64)` in `crates/shamir-query-types/src/read/filter/filter_value.rs`.

Acceptance:
1. `messagepack.rs` `decode_value` `Marker::U64` and the rmpv `Integer` arm return `InnerValue::Big(BigInt::from(u))` for `u > i64::MAX` and `Int` up to `i64::MAX`. Rewrite `codecs/README.md` (~line 279, "store as string") to match.
2. The lens decodes `0xcf` with `u > i64::MAX` into a new non-allocating `RecordValue` variant (for example `Big(u64)`) that behaves like the tree's `InnerValue::Big`: `record_value_to_kind` is `NonComparable`; `scalar_at` is `None`; `any_seq_elem` skips it; `get_str` / `get_str_owned` are `None`; `record_view_to_query_value` and `materialize_at` return `Big`; `RecordValue::Str` is never `Cow::Owned` afterwards. Update the docs at `record_value.rs`, `read_value.rs`, `filter_node.rs`. Audit every `Kind::Scalar` match and every exhaustive match on `RecordValue` in the engine (`fk_*` helpers, `filter_node`, `validator_db`): each new arm copies its `InnerValue::Big` counterpart.
3. `From<usize>` delegates to `From<u64>` (static assertion `usize::BITS <= 64`). A grep finds no `u64`/`usize` -> `as i64` conversion into `Value` or `FilterValue` in non-test src of shamir-types, shamir-query-types, shamir-query-builder.
4. `FilterValue` keeps its variant set, declaration order and `Serialize` output; `Float` uses a strict visitor accepting only `visit_f64`/`visit_f32`; `String` uses a `deserialize_any`-based visitor accepting strings plus `visit_u64(u > i64::MAX)` -> `u.to_string()`.
5. Stored-value parity tests (replace the "decimal text agrees" assertions in `record_view/tests/parity/parity_tests.rs` and `deintern_parity_tests.rs` with exact variant equality). Inputs: handcrafted `0xcf` bytes (not serialized Big, which is already text) as a top-level field, a nested map field and an array element; values `i64::MAX` (Int), `i64::MAX + 1`, `i64::MAX + 3`, `u64::MAX` (Big). Decoders that must agree: `QueryValue::from_bytes`, `InnerValue::from_bytes`, `msgpack_to_inner`, `msgpack_to_inner_legacy`, `RecordView::materialize_at`, `record_view_to_query_value` vs `inner_value_to_query_value`, `present_kind_at`/`scalar_at` lens vs tree. Compare with exact variant, not `assert_eq` across an encode/decode round trip (Big -> str -> Str is false by D2's design; use `numeric_eq`).
6. Filter parity tests in shamir-query-types: the same uint64 literal decoded through `rmp_serde::from_slice::<FilterValue>`, a direct `ReadQuery` / `DbRequest::CreateCursor`, `BatchOp::Read` and `serde_json` (napi path) equals `lit_u64(u)` (`String`), never `Float`, including inside `Array`; `i64::MAX` stays `Int`; `1.5`, `-0.0`, `NaN` stay `Float`.
7. `build_index_key_from_record` gives the same hash on a `RecordView` over raw uint64 bytes and on its `InnerValue::from_bytes` tree (pins that no index rebuild is needed).
8. Existing tests stay green unchanged: value_tests FG-1, query_value_serializer_tests, engine `u64_big_filter_match` predicates, `e2e-u64-big.test.ts`.

Compat: no wire or disk byte change (our own storage bytes never contain uint64 > i64::MAX). `RecordValue` gains a variant (workspace-internal, alpha): add a CHANGELOG entry. On the cursor path the decoded filter literal changes from a lossy `Float` to `String` (bug fix). Non-goal: the legacy `big:`/`dec:` prefixed decoders keep their fixture-pinned `Str` contract.


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
