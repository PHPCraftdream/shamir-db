# P1 stage H5 — p1-query-types-wire

## Goal

Query-types wire decode: _id, u64 filter, BatchOp dispatch, ids. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H5): Deserialized `InsertedRecord` exposes `_id`; u64 filter values above i64::MAX are not lossy floats; `BatchOp` dispatch is unambiguous and rejects unknown fields; ids serialize per spec; partial `limits` maps accepted; decode avoids triple round-trips; credential material is redacted in Debug.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-QUERY-TYPES-2D9F22F0 — `InsertedRecord::get_value_owned("_id")` returns `None` for every deserialized record, contradicting its own doc

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-types/src/ops/write/inserted_record.rs:98; crates/shamir-query-types/src/ops/write/inserted_record.rs:114; changed by: none
- R0 note (includes the fix site): OBSERVED: Deserialization stores the entire map in fields with id=None, while the _id accessor returns exclusively from id. INFERRED: A decoded _id cannot be retrieved through the documented accessor. Fix site: ops/write/inserted_record.rs, reconcile decoded _id storage with accessor lookup.

### RV-QUERY-TYPES-49564B25 — `FilterValue` silently coerces msgpack `uint64 > i64::MAX` to lossy `Float` — asymmetric with the crate's own u64 contract

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-types/src/read/filter/filter_value.rs:14; crates/shamir-query-types/src/wire/db_message.rs:265; changed by: none
- R0 note (includes the fix site): OBSERVED: CreateCursor directly deserializes ReadQuery into the i64-then-f64 untagged FilterValue; locally inspected pinned serde 1.0.229 forwards U64 to a float visitor using a cast. INFERRED: Oversized integers can still lose precision before evaluation; NumericKey changes cannot recover it. Fix site: read/filter/filter_value.rs, reject or exactly represent oversized unsigned inputs.

### RV-QUERY-TYPES-77BFC767 — `BatchOp` dispatch by key-presence + unknown-field tolerance can silently execute a different op than was sent

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-types/src/ops/batch/ops/batch_op.rs:296; crates/shamir-query-types/src/read/core/read_query.rs:12; changed by: none
- R0 note (includes the fix site): OBSERVED: from wins before insert_into, and ReadQuery has defaults without deny_unknown_fields. INFERRED: A map containing both discriminators still decodes as Read rather than being rejected as ambiguous. Fix site: ops/batch/ops/batch_op.rs, validate operation identity with awareness that update legitimately contains set.

### RV-QUERY-TYPES-A984DFED — `QueryResult::op_id` / `DdlOpStatus.op_id` / `request_id` serialize `RecordId` as raw `bin`, contradicting the crate's own base58-string wire convention and the `String`-typed poll request

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-types/src/types/ids/record_id.rs:163; crates/shamir-query-types/src/wire/db_message.rs:310; changed by: none
- R0 note (includes the fix site): OBSERVED: QueryResult/DdlOpStatus identifiers retain RecordId byte serialization, while GetDdlOpStatus requires a String; no field-level string codec bridges them. INFERRED: Response bytes cannot be reused directly as the poll token. Fix sites: read/core/query_result.rs and read/clause/ddl.rs, apply the agreed string wire representation consistently with request_id fields.

### RV-QUERY-TYPES-4BAE89B5 — `BatchLimits` rejects partial `limits` maps — the exact wire-compat failure #662 fixed for one field persists for the other five; and the limits are client-supplied

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/ops/batch/req/batch_limits.rs:32; crates/shamir-query-types/src/ops/batch/req/batch_limits.rs:123; changed by: none
- R0 note (includes the fix site): OBSERVED: The original five fields still lack serde defaults, while newer fields have defaults and clamped_to enforces numeric minima. INFERRED: A time-only limits map remains invalid; this is partial-map compatibility debt, not an unclamped-limit finding. Fix site: ops/batch/req/batch_limits.rs, define consistent partial-map defaulting.

### RV-QUERY-TYPES-1EC04E32 — `BatchOp::deserialize` — triple codec round-trip + key clones + linear dispatch chain per op

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/ops/batch/ops/batch_op.rs:271; crates/shamir-query-types/src/ops/batch/ops/batch_op.rs:286; changed by: none
- R0 note (includes the fix site): OBSERVED: Deserialization buffers QueryValue, clones keys, serializes MessagePack, then decodes through an ordered key-search chain. This confirms structural work, not a measured latency penalty or literally three complete round trips. Fix site: ops/batch/ops/batch_op.rs, replace the intermediate codec/dispatch pipeline.

### RV-QUERY-TYPES-FF6DD2A7 — `FilterValue` — 13-variant `#[serde(untagged)]` enum: content buffering + ~6 failed map-shaped trials per marker value

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/read/filter/filter_value.rs:9; crates/shamir-query-types/src/read/filter/filter_value.rs:77; changed by: none
- R0 note (includes the fix site): OBSERVED: All thirteen untagged alternatives remain; inspected pinned serde_derive 1.0.229 buffers Content and tries alternatives in order. This supports structural buffering/trial cost, not universal six failed map trials or measured latency. Fix site: read/filter/filter_value.rs, use explicit shape/marker dispatch while preserving wire compatibility.

### RV-QUERY-TYPES-1C72378C — Derived `Debug` on `DbRequest::ChangePasswordVerify` prints long-term SCRAM credential material

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/wire/db_message.rs:45; crates/shamir-query-types/src/wire/db_message.rs:194; changed by: none
- R0 note (includes the fix site): OBSERVED: DbRequest derives Debug and ChangePasswordVerify contains ordinary proof/stored-key/server-key byte vectors without redaction. INFERRED: A Debug consumer exposes those bytes; production logging is not established. Fix site: wire/db_message.rs, redact credential-bearing Debug output.

## Binding rulings

These decisions are final; do not re-litigate them.

### Binding ruling RU6 — wire form of `RecordId` in DDL results (RV-QUERY-TYPES-A984DFED)

Decided by 2x rush reviewer + @oh (3/3 on option A: canonical base58 string). One disagreement: @oh wanted a time-boxed lenient decode of the old 16-byte `bin` form; both reviewers said strict only. Final: **strict string only, NO legacy-bin decoder** (alpha, in-tree clients, no external users; a permanent dual input makes "canonical" meaningless). The disk-compat point from @oh is adopted in full.

RULING: the four wire-visible `RecordId` fields (`QueryResult::op_id`, `DdlOpStatus.op_id`, `DropIndexOp.request_id`, `RenameIndexOp.request_id`) are emitted as the canonical base58 `Display` string and decoded only from that string; `GetDdlOpStatus.op_id` stays a `String`. Implemented by a field-level serde adapter on exactly those four fields. `RecordId`'s own `Serialize`/`Deserialize` impl and the bincode disk encoding of the DDL op log stay byte-identical.

Facts: `RecordId` serializes as raw bytes (`crates/shamir-types/src/types/ids/record_id.rs` ~163), while `DbRequest::GetDdlOpStatus.op_id` is a `String` parsed by `FromStr` (`crates/shamir-query-types/src/wire/db_message.rs` ~310; `crates/shamir-db/src/shamir_db/shamir_db/core.rs` ~948), and the TS client types and sends it as a string. `CorruptRecordRef` (`crates/shamir-query-types/src/read/core/query_result.rs` ~74-110) already uses a base58 adapter whose doc calls it the convention for RecordId on the wire. `DdlOpStatus` is stored on disk as `bincode(status)` (`crates/shamir-index/src/base_index/infra/ddl_op_log.rs` ~91, ~131) — non-self-describing, so a global change would silently change stored bytes.

Acceptance:
1. Exact format: base58btc over the 16 raw bytes read big-endian, alphabet `123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz` (`crates/shamir-types/src/types/basic/base.rs`); each leading 0x00 byte becomes one `'1'`; length 16 to 22 characters; equals `RecordId::to_string()`. Decode is `RecordId::from_str` (rejects any decoded length other than 16 and any character outside the alphabet); test `from_str(s).to_string() == s` for every accepted input.
2. One shared adapter module in shamir-query-types (plain and `Option` forms) applied with `#[serde(with = ...)]` to exactly the four fields; existing `default` / `skip_serializing_if` attributes are kept (absent stays omitted). `CorruptRecordRef` and its adapter are not touched.
3. Serialize always emits a msgpack `str`; deserialize accepts only a canonical base58 `str` (any `bin`, non-canonical string or other type is an error). Tests cover accepted/rejected cases including going through the internally tagged `DbResponse::DdlOpStatus` envelope (content-buffered path).
4. `RecordId`'s own serde impl is unchanged. `ddl_op_log` encodes/decodes through a private disk mirror struct whose `op_id` uses `RecordId`'s native bytes serde; `DDL_OP_LOG_VERSION` stays 0x01. Golden test: the bincode bytes of a fixed `DdlOpStatus` equal a hex fixture captured from CURRENT master (take it from the existing code path before your change; say how), and that fixture decodes to the same value. No code path bincodes `DdlOpStatus` directly any more (grep).
5. Wire golden fixtures in shamir-query-types with a fixed RecordId (bytes 0x01..0x10): `rmp_serde::to_vec_named` of a `QueryResult` with `op_id`, of `DbResponse::DdlOpStatus`, and of `DropIndexOp` / `RenameIndexOp` with `request_id` produce exact pinned bytes; the value marker after each key is a str marker (fixstr 0xa0-0xbf), never 0xc4 (bin8).
6. Cross-language round trip (the test that would have caught this), at handler level in shamir-server through the real handler (`crates/shamir-server/src/engine/db_handler/handler.rs` ~885-905): run DROP INDEX, take the raw response msgpack bytes, decode them with a NON-Rust-typed mirror (`#[derive(Deserialize)] struct { op_id: String }`, how TS sees it), feed that string unchanged into a `get_ddl_op_status` request (encode, decode as `DbRequest`, handle) and assert `Some(status)` with `status.op_id` equal to the minted id and state `Succeeded`. Red on today's master. The same for `request_id`: send a base58 `request_id`, poll by the same string.
7. TS ts-unit test: decode the pinned golden bytes with `@msgpack/msgpack` and assert `typeof op_id === 'string'` matching `/^[1-9A-HJ-NP-Za-km-z]{16,22}$/`; assert `getDdlOpStatus(..., opId)` encodes `op_id` as a msgpack str using a fake transport.
8. Existing Rust tests (`ddl_tests.rs`, `p1_2_ddl_result_contract_tests.rs`) stay green without edits to their assertions.
9. Docs: add a "RecordId on the wire" entry to docs/guide-docs/client-server-protocol-spec/ (base58 string, the format above, the four fields, the poll token is the response string byte-for-byte) and a CHANGELOG entry (an old Rust client cannot decode a new server's str op_id: acceptable pre-v1).

Compat: wire: the four fields change from `bin` to `str` (what TS always declared and what the poll takes); disk: none; Node binding inherits; Rust client API unchanged (`op_id`/`request_id` stay `RecordId`, `get_ddl_op_status(&str)`). Out of scope (separate card): the TS `DdlOpState`/`DdlOpKind` shapes do not match Rust's externally tagged enums; the tests in items 6-7 assert only `op_id` and status presence.

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
