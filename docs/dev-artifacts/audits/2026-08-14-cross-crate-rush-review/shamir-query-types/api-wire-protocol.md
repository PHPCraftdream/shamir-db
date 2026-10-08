<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-types — api-wire-protocol revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

RecordId and InsertedRecord contract defects remain; direct FilterValue decoding still accepts lossy large integers, but normal BatchOp buffering normalizes them to exact decimal strings. Several alleged runtime defects are instead documented protocol/design choices.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 18 | 14 | 0 | 0 | 4 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- Parent inspection resolves the previously unavailable codec source: decoding is depth-bounded; universal stack-safety is still not claimed.

<a id="review-1"></a>

### Claim 1 — `InsertedRecord::get_value_owned("_id")` returns `None` for every deserialized record, contradicting its own doc

Status: `confirmed-open`. Current risk: `medium`.

The visitor retains _id only inside fields and the accessor ignores it when id=None. The contradiction and structural round-trip asymmetry remain. Normal BatchResponse deserialization uses QueryRecord::Direct, so not every protocol client loses its id accessor.

Evidence: [crates/shamir-query-types/src/write/inserted_record.rs:83](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L83); [crates/shamir-query-types/src/write/inserted_record.rs:98](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L98); [crates/shamir-query-types/src/write/inserted_record.rs:114](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L114); [crates/shamir-query-types/src/read/query_record.rs:151](../../../../../crates/shamir-query-types/src/read/query_record.rs#L151).

<a id="review-2"></a>

### Claim 2 — `FilterValue` silently coerces msgpack `uint64 > i64::MAX` to lossy `Float` — asymmetric with the crate's own u64 contract

Status: `confirmed-open`. Current risk: `medium`.

Direct untagged FilterValue decoding tries Int then permissive Float; pinned serde accepts u64 through the float visitor's cast. This is reachable in directly decoded filters, including CreateCursor. Normal BatchOp decode first promotes QueryValue u64 to Big and re-encodes an exact decimal string, invalidating the original Execute example.

Evidence: [crates/shamir-query-types/src/filter/filter_value.rs:9](../../../../../crates/shamir-query-types/src/filter/filter_value.rs#L9); [crates/shamir-query-types/src/wire/db_message.rs:222](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L222); [crates/shamir-query-types/src/batch/batch_op.rs:262](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L262); [crates/shamir-types/src/types/value.rs:73](../../../../../crates/shamir-types/src/types/value.rs#L73); [crates/shamir-types/src/types/value.rs:142](../../../../../crates/shamir-types/src/types/value.rs#L142); [Cargo.lock:3204](../../../../../Cargo.lock#L3204); [docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md:28](../../../../../docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md#L28).

<a id="review-3"></a>

### Claim 3 — `BatchOp` dispatch by key-presence + unknown-field tolerance can silently execute a different op than was sent

Status: `confirmed-open`. Current risk: `medium`.

First-present-key dispatch still accepts a map containing both from and another operation discriminator as Read, whose other fields default and unknown fields are ignored. The suggested blanket one-discriminator-field invariant must allow intentional UpdateOp.set overlap.

Evidence: [crates/shamir-query-types/src/batch/batch_op.rs:286](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L286); [crates/shamir-query-types/src/batch/batch_op.rs:433](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L433); [crates/shamir-query-types/src/read/read_query.rs:12](../../../../../crates/shamir-query-types/src/read/read_query.rs#L12); [crates/shamir-query-types/src/write/types.rs:114](../../../../../crates/shamir-query-types/src/write/types.rs#L114).

<a id="review-4"></a>

### Claim 4 — The same `RecordId` identifier rides the wire three different ways (`op_id` bin vs `op_id` string vs `after_id`/`_id` base58)

Status: `confirmed-open`. Current risk: `medium`.

Response/correlation RecordId fields still serialize as bytes while the poll endpoint and TS interfaces require strings; existing base58 adapters remain unused on these fields.

Evidence: [crates/shamir-query-types/src/read/query_result.rs:190](../../../../../crates/shamir-query-types/src/read/query_result.rs#L190); [crates/shamir-query-types/src/read/ddl.rs:14](../../../../../crates/shamir-query-types/src/read/ddl.rs#L14); [crates/shamir-query-types/src/wire/db_message.rs:275](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L275); [crates/shamir-client-ts/src/core/types/batch.ts:269](../../../../../crates/shamir-client-ts/src/core/types/batch.ts#L269).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — HMAC canonical inputs are not injective (NUL/comma/slash aliasing) and don't cover `cascade`/`if_exists`/`replace` modifiers

Status: `confirmed-open`. Current risk: `medium`.

NUL/slash/CSV joins remain ambiguous and the cited modifiers remain absent. Such changes preserve tags because canonicalizers do not receive the modifiers. This is narrower intent binding, not an authorization break.

Evidence: [crates/shamir-query-types/src/hmac.rs:89](../../../../../crates/shamir-query-types/src/hmac.rs#L89); [crates/shamir-query-types/src/hmac.rs:184](../../../../../crates/shamir-query-types/src/hmac.rs#L184); [crates/shamir-query-types/src/hmac.rs:357](../../../../../crates/shamir-query-types/src/hmac.rs#L357); [crates/shamir-query-types/src/admin/types/function_ops.rs:34](../../../../../crates/shamir-query-types/src/admin/types/function_ops.rs#L34); [crates/shamir-query-types/src/admin/types/db_ops.rs:35](../../../../../crates/shamir-query-types/src/admin/types/db_ops.rs#L35).

Grouping/duplicate: `security-crypto.md#5,#6`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — Closed vocabularies modeled as raw `String` where typed enums are the crate's own established pattern

Status: `confirmed-open`. Current risk: `low`.

The cited isolation, status, FTS/computed, index/function/schema fields and error codes remain strings. This is mixed API-design debt: some invalid strings intentionally default, and extensible function/error vocabularies are not necessarily closed sets.

Evidence: [crates/shamir-query-types/src/batch/batch_request.rs:64](../../../../../crates/shamir-query-types/src/batch/batch_request.rs#L64); [crates/shamir-query-types/src/batch/transaction_info.rs:12](../../../../../crates/shamir-query-types/src/batch/transaction_info.rs#L12); [crates/shamir-query-types/src/filter/filter_enum.rs:163](../../../../../crates/shamir-query-types/src/filter/filter_enum.rs#L163); [crates/shamir-query-types/src/admin/types/index_ops.rs:65](../../../../../crates/shamir-query-types/src/admin/types/index_ops.rs#L65); [crates/shamir-query-types/src/wire/db_message.rs:338](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L338).

<a id="review-7"></a>

### Claim 7 — Depth/nesting caps are post-deserialization checks, but serde deserialization itself recurses unbounded

Status: `refuted`. Current risk: —.

Parent source check of the checksummed rmp-serde 1.3.1 archive finds a default depth counter of 1024, decremented for arrays/maps and rejected with DepthLimitExceeded before unbounded descent. The categorical missing-bound / arbitrary-depth decode mechanism is refuted, not fixed. No project override increases that limit. This does not prove 1024 recursive containers are stack-safe on every target; a lower project limit is separate, unmeasured hardening.

Evidence: [crates/shamir-query-types/src/filter/filter_enum.rs:12](../../../../../crates/shamir-query-types/src/filter/filter_enum.rs#L12); [crates/shamir-query-types/src/batch/batch_op.rs:262](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L262); [crates/shamir-server/src/db_handler/handler.rs:344](../../../../../crates/shamir-server/src/db_handler/handler.rs#L344); [Cargo.lock:2949](../../../../../Cargo.lock#L2949).

Pinned dependency evidence: [rmp-serde 1.3.1, src/decode.rs:294](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs); [rmp-serde 1.3.1, src/decode.rs:566](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs).

Grouping/duplicate: `security-crypto.md#1`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — `BatchLimits` rejects partial `limits` maps — the exact wire-compat failure #662 fixed for one field persists for the other five; and the limits are client-supplied

Status: `confirmed-open`. Current risk: `medium`.

Only max_iterations has a field default; supplying a limits map still requires the other five fields. Omission of the entire limits object uses Default. Security authority claims remain overstated, although runtime iteration count now has an engine ceiling.

Evidence: [crates/shamir-query-types/src/batch/batch_limits.rs:31](../../../../../crates/shamir-query-types/src/batch/batch_limits.rs#L31); [crates/shamir-query-types/src/batch/batch_limits.rs:68](../../../../../crates/shamir-query-types/src/batch/batch_limits.rs#L68); [crates/shamir-query-types/src/batch/batch_request.rs:98](../../../../../crates/shamir-query-types/src/batch/batch_request.rs#L98); [crates/shamir-query-types/src/batch/tests/batch_limits_tests.rs:26](../../../../../crates/shamir-query-types/src/batch/tests/batch_limits_tests.rs#L26).

<a id="review-9"></a>

### Claim 9 — `$query` path syntax silently reserves `.count`/`.length` — record fields with those names are unreachable

Status: `refuted`. Current risk: —.

The standalone parser reserves these names explicitly in its documentation, but current execution does not use that parser: scalar/column resolvers look up the literal field suffix, including count/length. Therefore the claimed runtime unreachable-field and row-count substitution scenario is false.

Evidence: [crates/shamir-query-types/src/batch/reference.rs:16](../../../../../crates/shamir-query-types/src/batch/reference.rs#L16); [crates/shamir-query-types/src/batch/reference.rs:186](../../../../../crates/shamir-query-types/src/batch/reference.rs#L186); [crates/shamir-engine/src/query/filter/resolve.rs:742](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L742); [crates/shamir-engine/src/query/filter/resolve.rs:751](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L751); [crates/shamir-engine/src/query/filter/resolve.rs:800](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L800).

<a id="review-10"></a>

### Claim 10 — `InsertedRecord` with a non-map `fields` and no `id` serializes to a shape its own deserializer rejects

Status: `confirmed-open`. Current risk: `low`.

The serializer emits a scalar directly for id=None/non-map fields, but deserialization requires a map. The registered scalar test checks serialization only, so cannot detect the asymmetric decode.

Evidence: [crates/shamir-query-types/src/write/inserted_record.rs:63](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L63); [crates/shamir-query-types/src/write/inserted_record.rs:102](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L102); [crates/shamir-query-types/src/write/tests/inserted_record_tests.rs:220](../../../../../crates/shamir-query-types/src/write/tests/inserted_record_tests.rs#L220); [crates/shamir-query-types/src/write/tests/mod.rs:2](../../../../../crates/shamir-query-types/src/write/tests/mod.rs#L2).

<a id="review-11"></a>

### Claim 11 — `QueryRecord` wire shape aliases `Direct(QueryValue::Bin)` and `IdBytes`; `as_value()` silently substitutes `Null` for opaque rows

Status: `confirmed-open`. Current risk: `low`.

Binary payloads still reconstruct IdBytes, and as_value still yields Null for that variant. These are explicitly documented and tested representation limitations, not proven breaches of a promised variant-preserving round trip or hidden runtime behavior.

Evidence: [crates/shamir-query-types/src/read/query_record.rs:87](../../../../../crates/shamir-query-types/src/read/query_record.rs#L87); [crates/shamir-query-types/src/read/query_record.rs:184](../../../../../crates/shamir-query-types/src/read/query_record.rs#L184); [crates/shamir-query-types/src/read/tests/query_record_tests.rs:77](../../../../../crates/shamir-query-types/src/read/tests/query_record_tests.rs#L77).

<a id="review-12"></a>

### Claim 12 — Wire tag conventions are inconsistent across the protocol

Status: `confirmed-open`. Current risk: `low`.

The enumerated tag keys, casings, and external tagging remain. This is protocol consistency/ergonomics debt; no requirement establishes that differing discriminators are a runtime defect.

Evidence: [crates/shamir-query-types/src/read/limit.rs:14](../../../../../crates/shamir-query-types/src/read/limit.rs#L14); [crates/shamir-query-types/src/read/select.rs:50](../../../../../crates/shamir-query-types/src/read/select.rs#L50); [crates/shamir-query-types/src/wire/repl.rs:25](../../../../../crates/shamir-query-types/src/wire/repl.rs#L25); [crates/shamir-query-types/src/subscribe/deliver_mode.rs:8](../../../../../crates/shamir-query-types/src/subscribe/deliver_mode.rs#L8).

<a id="review-13"></a>

### Claim 13 — `FieldPath` accepts a bare string in filters but requires arrays in SELECT/ORDER BY/GROUP BY/aggregate field

Status: `confirmed-open`. Current risk: `low`.

The custom string-or-array deserializer remains filter-local; the cited projection/order/group/aggregate fields still use Vec-based FieldPath directly.

Evidence: [crates/shamir-query-types/src/filter/filter_enum.rs:370](../../../../../crates/shamir-query-types/src/filter/filter_enum.rs#L370); [crates/shamir-query-types/src/read/select.rs:57](../../../../../crates/shamir-query-types/src/read/select.rs#L57); [crates/shamir-query-types/src/read/order_by.rs:38](../../../../../crates/shamir-query-types/src/read/order_by.rs#L38); [crates/shamir-query-types/src/read/group_by.rs:11](../../../../../crates/shamir-query-types/src/read/group_by.rs#L11); [crates/shamir-query-types/src/read/agg.rs:25](../../../../../crates/shamir-query-types/src/read/agg.rs#L25).

<a id="review-14"></a>

### Claim 14 — `InsertOp` carries two parallel record channels with unspecified result ordering

Status: `confirmed-open`. Current risk: `low`.

The DTO field documentation still omits combined result order. Current engine implementation and its comments positively establish values-first, then idmsgpack; nondeterministic or undefined actual ordering is not established.

Evidence: [crates/shamir-query-types/src/write/types.rs:81](../../../../../crates/shamir-query-types/src/write/types.rs#L81); [crates/shamir-engine/src/table/write_exec.rs:331](../../../../../crates/shamir-engine/src/table/write_exec.rs#L331); [crates/shamir-engine/src/table/write_exec.rs:407](../../../../../crates/shamir-engine/src/table/write_exec.rs#L407); [crates/shamir-engine/src/table/write_exec.rs:418](../../../../../crates/shamir-engine/src/table/write_exec.rs#L418).

<a id="review-15"></a>

### Claim 15 — `query_version` negotiation coverage is inconsistent within `DbRequest`

Status: `refuted`. Current risk: —.

The documented version is for incompatible BatchRequest/query-language schema changes, not every application operation. Commit/rollback carry an already-open handle rather than a new batch; replication is independently versioned. A hypothetical future admin schema change is not a present defect.

Evidence: [crates/shamir-query-types/src/wire/db_message.rs:12](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L12); [crates/shamir-query-types/src/wire/db_message.rs:89](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L89); [crates/shamir-query-types/src/wire/db_message.rs:119](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L119); [crates/shamir-query-types/src/wire/db_message.rs:133](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L133); [crates/shamir-server/src/db_handler/tx_handlers.rs:109](../../../../../crates/shamir-server/src/db_handler/tx_handlers.rs#L109).

<a id="review-16"></a>

### Claim 16 — "Always required" HMAC fields are `Option<String>` — required-ness exists only in prose and runtime gates

Status: `refuted`. Current risk: —.

Optional raw DTO fields are intentional and also specified in AUTH_PROTOCOL. All three unconditional handlers explicitly reject None with hmac_required before mutation. A validated wrapper could improve ergonomics, but no missing mandatory runtime gate is shown.

Evidence: [crates/shamir-query-types/src/auth/types.rs:218](../../../../../crates/shamir-query-types/src/auth/types.rs#L218); [crates/shamir-server/src/db_handler/admin.rs:121](../../../../../crates/shamir-server/src/db_handler/admin.rs#L121); [crates/shamir-server/src/db_handler/admin.rs:296](../../../../../crates/shamir-server/src/db_handler/admin.rs#L296); [crates/shamir-server/src/db_handler/admin.rs:377](../../../../../crates/shamir-server/src/db_handler/admin.rs#L377); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:801](../../../../../docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L801).

<a id="review-17"></a>

### Claim 17 — Doc/wire mismatches and stale references

Status: `confirmed-open`. Current risk: `nit`.

All cited components persist: duplicate fk_restrict, split HMAC table, nested validator examples incompatible with flat structs, missing nesting default row, stale has_next_hint name, and no wire-root CURRENT_REPL_PROTO_VER re-export. The latter is discoverability debt, not a broken wire format.

Evidence: [crates/shamir-query-types/src/wire/db_message.rs:330](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L330); [crates/shamir-query-types/src/hmac.rs:61](../../../../../crates/shamir-query-types/src/hmac.rs#L61); [crates/shamir-query-types/src/admin/types/validator_ops.rs:59](../../../../../crates/shamir-query-types/src/admin/types/validator_ops.rs#L59); [crates/shamir-query-types/src/batch/batch_limits.rs:13](../../../../../crates/shamir-query-types/src/batch/batch_limits.rs#L13); [crates/shamir-query-types/src/read/limit.rs:320](../../../../../crates/shamir-query-types/src/read/limit.rs#L320); [crates/shamir-query-types/src/wire/mod.rs:20](../../../../../crates/shamir-query-types/src/wire/mod.rs#L20).

<a id="review-18"></a>

### Claim 18 — Inline `#[cfg(test)] mod tests` in implementation files, despite the documented `tests/` layout and existing sibling test files

Status: `confirmed-open`. Current risk: `low`.

Both inline modules and sibling registered test files remain. The violation is organizational, not evidence that the tests are unreachable.

Evidence: [crates/shamir-query-types/src/read/query_record.rs:302](../../../../../crates/shamir-query-types/src/read/query_record.rs#L302); [crates/shamir-query-types/src/write/inserted_record.rs:135](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L135); [crates/shamir-query-types/src/read/tests/mod.rs:3](../../../../../crates/shamir-query-types/src/read/tests/mod.rs#L3); [crates/shamir-query-types/src/write/tests/mod.rs:2](../../../../../crates/shamir-query-types/src/write/tests/mod.rs#L2).

Grouping/duplicate: `style-claude-md.md#2`. This row is not another independent defect.

## Corrections and qualified non-findings

- The numeric wire contract deliberately represents large FilterValue literals as exact decimal strings. A lossless fix should preserve that contract rather than assume a new UInt/Big variant is mandatory.
- Normal Execute/TxExecute BatchOp buffering already normalizes raw large u64 values to strings; direct Filter/ReadQuery decoding is the remaining lossy path.
- The proposed discriminator-field intersection invariant is invalid without exceptions: UpdateOp intentionally contains both update and set.
- Unknown-string rejection is not universally additive-safe; vector quantization explicitly documents fallback for unrecognized values, and function/error vocabularies can be extensible.
- count/length runtime field access works through the actual resolvers; the standalone QueryReference parser's reservation is documented.
- QueryRecord Null sentinel behavior is already documented and pinned; stronger accessor types would be an API redesign.
- Actual mixed insert ordering is deterministic and documented in the engine; only the DTO-facing documentation remains incomplete.
- Unversioned non-batch operations and optional raw HMAC fields do not demonstrate current protocol/security defects.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-types -- API & wire-protocol design

## Summary

The crate is a well-documented, msgpack-first DTO layer with strong round-trip test coverage and genuinely good forward-compat hygiene (`skip_serializing_if` + `serde(default)` additive fields, the #983 Binary/String untagged fix, base58 RecordId conventions where applied). However, the two central value types have real wire-correctness defects: `InsertedRecord`'s deserializer loses the `_id` field its own accessor and doc promise to expose, and `FilterValue`'s untagged decode silently coerces `uint64 > i64::MAX` to lossy `Float` on the request side while the response side (`QueryRecord`) implements the documented lossless `Big` contract. Beyond those, the protocol leans heavily on stringly-typed vocabularies, unenforced discriminator-key uniqueness in `BatchOp` dispatch, and HMAC canonical forms that are not injective — all patterns that work today only through review discipline rather than type structure.

## Findings

### 1. `InsertedRecord::get_value_owned("_id")` returns `None` for every deserialized record, contradicting its own doc
- **File:** `crates/shamir-query-types/src/write/inserted_record.rs:81-104` (deserialize), `:114-119` (accessor)
- **Severity:** high
- **Issue:** Serialize injects `_id` (base58) into the map from `self.id`; Deserialize never extracts it back — it decodes the whole map into `fields` with `id: None` (line 98). The deserializer's doc (lines 83-85) explicitly claims "The `_id` key is stored in `fields` when present; callers can look it up via `get_value_owned(\"_id\")`" — but `get_value_owned` short-circuits on `key == "_id"` and returns `self.id.as_ref().map(...)`, which is `None` after any deserialization; it never falls through to `self.fields.get("_id")`. Round-trip also breaks structural equality: `InsertedRecord { id: Some(r), fields }` round-trips to `{ id: None, fields-with-_id-inside }`.
- **Failure scenario:** A client deserializes a `WriteResult`, then calls `record.get_value_owned("_id")` (e.g. to feed `Pagination::After::after_id`, whose doc says to echo "the `_id` of the last row") — it silently gets `None` for every row even though `_id` is right there in `fields`. The documented access path is dead on exactly the side (client) that needs it.
- **Suggested fix:** In `get_value_owned`, fall through to `self.fields.get("_id")` when `self.id` is `None`; better, have the deserializer extract `_id` from the map back into `id: Option<RecordId>` so the round-trip is symmetric. Fix the stale doc either way.

### 2. `FilterValue` silently coerces msgpack `uint64 > i64::MAX` to lossy `Float` — asymmetric with the crate's own u64 contract
- **File:** `crates/shamir-query-types/src/filter/filter_value.rs:9-81` (untagged, `Int(i64)` before `Float(f64)`, no `UInt`/`Big` variant)
- **Severity:** high
- **Issue:** For an untagged enum, serde tries variants in order: `Int(i64)` rejects a uint64 above `i64::MAX`, then `Float(f64)` accepts it via a lossy `as f64`. The crate already solved this exact problem on the response side — `QueryRecord`'s `visit_u64` (`read/query_record.rs:105-115`, tested up to `u64::MAX` in `read/tests/query_record_tests.rs:281-294`) promotes losslessly to `QueryValue::Big`, and `Cargo.toml:20-24` documents the "unified u64 contract". The request-side value type has no such handling and no test (`filter/tests/filter_value_conv_tests.rs` covers only `i64::MAX`).
- **Failure scenario:** A client (TS `BigInt` via `@msgpack/msgpack`, or any raw encoder) sends `{"op":"eq","field":"id","value":18446744073709551615}` as msgpack uint64. It decodes to `Float(1.8446744073709552e19)`; the equality filter against the stored u64 then never matches (or float-compares to the wrong rows) — silent wrong results, no error anywhere.
- **Suggested fix:** Add a `UInt(u64)` (or `Big(BigInt)`) variant declared before `Float`, mirroring `QueryRecord`'s contract — or, if the wire vocabulary must stay frozen, give `Float` a strict deserializer that rejects integer payloads that don't round-trip exactly (the same technique `de_binary_strict` used for `Binary` in #983). Add a `u64::MAX` wire round-trip test.

### 3. `BatchOp` dispatch by key-presence + unknown-field tolerance can silently execute a different op than was sent
- **File:** `crates/shamir-query-types/src/batch/batch_op.rs:286-438` (dispatch), `:287-288` (`has("from")` first), `read/read_query.rs:12-46` (every `ReadQuery` field but `from` is defaulted)
- **Severity:** medium
- **Issue:** Dispatch selects the first if-chain arm whose key is *present*, then decodes with the op struct's serde impl — which ignores unknown fields (no `deny_unknown_fields` anywhere in the crate; verifiable by grep). Because `ReadQuery` succeeds on any map containing `"from"` and defaults everything else, any payload whose key set contains an earlier discriminator (`from`, `insert_into`, `update`, `delete_from`, …) is decoded as that op with its remaining fields silently dropped. `QueryEntry`'s `#[serde(flatten)]` (`query_entry.rs:39-40`) makes this worse: unknown sibling keys (e.g. a typo'd `return_resultt`) are forwarded into the dispatch map and swallowed. Nothing enforces discriminator-key uniqueness across the ~70 op structs; the `"set"`-last comment (line 433-434) shows the scheme already needed manual ordering patches.
- **Failure scenario:** A future op struct gains a non-discriminator field named `from`, `update`, `set`, or `list` (or a third-party client sends `{... "from": ...}` intending a new op): the payload decodes as `Read` over table `"from"` with all other fields dropped — a different operation runs, silently, instead of the client getting "Unknown operation type".
- **Suggested fix:** Add a compile-time or unit-test invariant: for every op struct, its field-name set must intersect the discriminator list exactly at its own discriminator (a `static_assert`-style macro or a generated test walking all ops). Longer term, wrap ops in an explicit single-key envelope (as `ListOp`/`ReplRequest` already do) instead of bare struct merging. At minimum, make dispatch verify the payload's key set is *exactly* the chosen op's field set modulo known additive fields.

### 4. The same `RecordId` identifier rides the wire three different ways (`op_id` bin vs `op_id` string vs `after_id`/`_id` base58)
- **File:** `crates/shamir-query-types/src/read/query_result.rs:190` and `read/ddl.rs:14` (`RecordId`, derived serde → raw 16-byte msgpack `bin`), `admin/types/index_ops.rs:125,154` (`request_id` same), vs `wire/db_message.rs:267-276` (`GetDdlOpStatus { op_id: String }`), vs the crate's own stated convention in `read/query_result.rs:74-80` ("base58 string … NOT raw msgpack bytes, despite RecordId's own derived Serialize")
- **Severity:** medium
- **Issue:** A client that receives `QueryResult::op_id` (or `DdlOpStatus.op_id`, `RenameIndexOp::request_id`) and needs to poll `GetDdlOpStatus` cannot echo it: the response gives raw `bin`, the request expects a base58 `String`. The base58 bridging module (`id_as_base58_string`, `query_result.rs:98-110`) and `opt_record_id_base58` (`read/limit.rs:142-167`) already exist — they're just not applied consistently.
- **Failure scenario:** Crash-recovery polling — the flagship use case these fields exist for — requires the client to know to hex/base58-render a binary blob the DTO gave it as opaque `RecordId`; a TS client that round-trips the field as-is sends bytes where a string is required and the poll fails to parse.
- **Suggested fix:** Serialize every wire-visible `RecordId` field through the existing base58 modules; keep `RecordId`'s derived serde confined to storage-internal contexts.

### 5. HMAC canonical inputs are not injective (NUL/comma/slash aliasing) and don't cover `cascade`/`if_exists`/`replace` modifiers
- **File:** `crates/shamir-query-types/src/hmac.rs:89-99` (`join_null`, unescaped), `:184-196` (`canonical_resource_ref`, `/`-joined), `:357-364` (grants CSV-joined), `:402-408` (`create_scram_user`: name `"a\0b"` with no roles ≡ name `"a"` + role `"b"`); `admin/types/db_ops.rs:35` / `repo_ops.rs:34` / `table_ops.rs:54` (`cascade` not in canonical form)
- **Severity:** medium
- **Issue:** The module's stated contract is "Matching tag = confirmation of intent" (lines 14-15), but the canonical byte strings are ambiguous: parts are joined with `\0` without escaping and no DTO validates names NUL-free, `ResourceRef` renders with unescaped `/`, and `secret_grants` are comma-joined (a grant containing `,` aliases a different grant list). Separately, the tag for `drop_db`/`drop_repo`/`drop_table` hashes only the names — a tag computed for a plain drop confirms the `cascade: true` variant (strictly larger blast radius) with identical bytes.
- **Failure scenario:** A tool that signs "drop table X" and shows the user that exact intent can have the same bytes replayed against `drop_table X cascade=true`; a username containing `\0` makes a `create_scram_user` tag alias two different (name, roles) intents. Both are intent-confirmation degradations rather than auth breaks (the doc is honest that TLS+SCRAM carry authn), but nothing in the DTO layer prevents the ambiguous names.
- **Suggested fix:** Length-prefix canonical parts (or escape `\0`/`/`/`,`), include boolean modifiers in the canonical form, and validate name strings NUL-free at the DTO boundary so ambiguity can't be constructed.

### 6. Closed vocabularies modeled as raw `String` where typed enums are the crate's own established pattern
- **File:** instances across `batch/batch_request.rs:64-81` (`isolation`, `durability`), `wire/db_message.rs:99-100` (`TxBegin::isolation`), `batch/transaction_info.rs:12` (`status`), `filter/filter_enum.rs:163` (`Fts.mode`), `filter/filter_enum.rs:190-198` (`Computed.expr_op`/`cmp` — while the sibling `ValueCompare` variant uses the typed `ValueCompareOp` at `:205-214`), `admin/types/index_ops.rs:39-71` (`index_type`, `fts_tokenizer`, `vector_metric`, `vector_quantization`, `functional_op`), `admin/types/function_ops.rs:42-47` (`visibility`, `security`), `admin/types/schema_ops.rs:40` (`r#type`), `:211` (`CompareDto.op`), `wire/db_message.rs:316-339` / `wire/repl.rs:97-105` (`Error.code` vocabulary lives only in doc comments)
- **Severity:** medium
- **Issue:** The crate defines typed, snake_case serde enums for exactly this purpose (`ReplDirection`, `ReplMode`, `EventMask`, `AggFunc`, `FkAction`, `ValueCompareOp`, `OrderDirection`, `ResultEncoding`) — but a large fraction of closed vocabularies are `Option<String>`/`String`. Typos fail only server-side (or silently default); clients cannot exhaustively match; renames are invisible to the compiler. The `Computed.cmp` case is self-inconsistent within one file. Wire error codes (`hmac_required`, `cursor_not_found`, `fk_*`…) are documented in prose with no shared constants, so server emitters and client matchers can drift undetected.
- **Failure scenario:** `"serialziable"`, `"cosine"`, `"definer"` typo'd by a hand-rolled client: best case an opaque runtime error deep in the engine; worst case (fields with `#[serde(default)]` fallback semantics) a silently different isolation/metric/security level than intended.
- **Suggested fix:** Convert closed sets to serde enums (additive-safe: unknown-value rejection is the desired behavior for closed sets); for the error-code channel, publish `pub const` code strings in this crate so both server and client match against one source.

### 7. Depth/nesting caps are post-deserialization checks, but serde deserialization itself recurses unbounded
- **File:** `crates/shamir-query-types/src/filter/filter_enum.rs:7-9,219-238` (`MAX_FILTER_DEPTH` enforced only by opt-in `check_filter_depth` *after* `Deserialize`, whose `Box<Filter>` chain recurses per input level); `batch/batch_op.rs:256-277` (`BatchOp::deserialize` → `SubBatchOp` → `BatchRequest` → `QueryEntry` → `BatchOp` recursion; `max_nesting_depth` is plan-time, `batch/planner.rs:109-117`)
- **Severity:** medium
- **Issue:** `MAX_FILTER_DEPTH`'s doc says deep filters are "rejected to prevent stack overflow post-handshake" — but the overflow happens *during* `Filter`/`BatchOp`/`QueryValue` deserialization, before any guard runs. Every ingress point must independently remember to call `check_filter_depth` / the planner; the DTO itself neither bounds nor checks recursion. (Auth-gated: requires a valid SCRAM session.)
- **Failure scenario:** An authenticated client sends a ~10⁴–10⁵-deep chain of `{"batch": {"batch": …}}` or `not` wrappers — a few-KB payload — and the deserialization recursion overflows the thread stack before `BatchLimits` is ever consulted.
- **Suggested fix:** Enforce depth *during* decode: a counting `Deserializer` wrapper (or a cheap depth pre-pass over the buffered `QueryValue` in `BatchOp::deserialize`, which already buffers the whole map) that fails fast above a hard cap; then the existing semantic checks stay as-is.

### 8. `BatchLimits` rejects partial `limits` maps — the exact wire-compat failure #662 fixed for one field persists for the other five; and the limits are client-supplied
- **File:** `crates/shamir-query-types/src/batch/batch_limits.rs:31-69` (only `max_iterations` has `#[serde(default = "...")]`), `batch/batch_request.rs:97-99`, `batch/planner.rs:109-117`
- **Severity:** medium
- **Issue:** The `max_iterations` doc (lines 61-66) describes precisely how a mandatory field breaks older/partial `limits` payloads ("missing field `max_iterations`") — yet `max_queries`, `max_dependency_depth`, `max_execution_time_secs`, `max_result_size`, `max_nesting_depth` remain mandatory, so a client wanting to lower one knob must send all six. Additionally, the struct is documented as "security limits… prevents DoS" while being a client-authoritative request field, and the planner's own comment documents the budget as per-level (worst case `max_queries^max_nesting_depth`).
- **Failure scenario:** TS client sends `{"limits": {"max_execution_time_secs": 5}}` → rejected with `missing field max_queries`; conversely a client can *raise* every limit unless the server clamps elsewhere (not visible in this crate).
- **Suggested fix:** Give every field a `#[serde(default = "...")]` matching `Default` (same as #662); rename the doc to "client-requested limits (server clamps)" so the wire contract doesn't over-claim security.

### 9. `$query` path syntax silently reserves `.count`/`.length` — record fields with those names are unreachable
- **File:** `crates/shamir-query-types/src/batch/reference.rs:186-191` (field named `count`/`length` unconditionally becomes `QueryPath::Count`)
- **Severity:** medium
- **Issue:** `@orders[].length` does not extract each row's `length` field; it returns the result count. There is no escape hatch (brackets are index-only; dots are field-only), and the substitution is silent — no error, just different data. `count` and `length` are common column names.
- **Failure scenario:** A table with a `count` or `length` column; any `$query` reference to it returns the row count instead of the column value — silent wrong results in dependent ops.
- **Suggested fix:** Either reserve the names loudly (plan-time error when the referenced alias's projection contains a field with the magic name) or add an unambiguous field syntax (e.g. `["length"]` bracketed field segments) and keep `.count`/`.length` as sugar.

### 10. `InsertedRecord` with a non-map `fields` and no `id` serializes to a shape its own deserializer rejects
- **File:** `crates/shamir-query-types/src/write/inserted_record.rs:63-74` (serialize falls through to `fields.serialize`) vs `:102` (`deserialize_map` only)
- **Severity:** low
- **Issue:** `InsertedRecord { id: None, fields: QueryValue::Str("x") }` serializes as a bare msgpack string; deserializing those bytes fails with "expected a map". Round-trip is not total; the `{id: Some, non-map}` case works only because the `{"_id":…,"_value":…}` envelope happens to be a map (and then decodes with `_id`/`_value` stranded inside `fields`).
- **Suggested fix:** Always emit the two-key envelope when `fields` is not a map, or make the visitor accept scalars (`deserialize_any` + non-map arm wrapping into `fields`).

### 11. `QueryRecord` wire shape aliases `Direct(QueryValue::Bin)` and `IdBytes`; `as_value()` silently substitutes `Null` for opaque rows
- **File:** `crates/shamir-query-types/src/read/query_record.rs:43,58-67,87-92,189-196`
- **Severity:** low
- **Issue:** Deserialization routes every msgpack `bin` payload to `IdBytes`, so `Direct(Bin(x))` round-trips as `IdBytes(x)` — variant identity is not preserved (top-level bin rows are indistinguishable from id-keyed pass-through bytes). And `as_value()` on `IdBytes` returns `QueryValue::Null` as a "safe sentinel": a caller that forwards `as_value()` output (serialize, map into another op) silently turns a real row into `Null` rather than failing.
- **Suggested fix:** Document (or forbid via the builder) top-level binary rows on the Name path, and make `as_value()` on `IdBytes` return `Option`/`Result` (or panic-free explicit `de_intern` API) instead of inventing a `Null` row.

### 12. Wire tag conventions are inconsistent across the protocol
- **File:** `read/limit.rs:14-15` (`tag = "mode"`, no `rename_all` → PascalCase `LimitOffset`/`Page`/`After`/`None`, self-acknowledged at `:41-43`) vs snake_case tags in `filter_enum.rs:13` (`op`), `wire/db_message.rs:30,281` (`op`/`kind`), `select.rs:50` (`type`), `wire/repl.rs:25,70` (`repl_op`/`repl_kind`), `auth/types.rs:15` (`scope`); `At` externally tagged (`temporal.rs:12-18`); `DeliverMode` externally tagged mixing bare strings and single-key maps (`subscribe/deliver_mode.rs`)
- **Severity:** low
- **Issue:** Four different tag key names and two casings for variant tags in one protocol. Not a correctness bug, but a permanent tax on hand-writing clients, protocol docs, and any codegen — and it makes "add an enum" decisions ad hoc each time.
- **Suggested fix:** Standardize on one tag key (e.g. `"op"`) + `snake_case` for new enums; schedule `Pagination`'s PascalCase tags for deprecation-by-alias (accept both, emit canonical).

### 13. `FieldPath` accepts a bare string in filters but requires arrays in SELECT/ORDER BY/GROUP BY/aggregate field
- **File:** `filter/filter_enum.rs:17` + `:251-265` (`de_field_path`: string-or-array) vs `read/select.rs:57` (`SelectItem::Field.path`), `read/order_by.rs:38`, `read/group_by.rs:11`, `read/agg.rs:23` (plain `FieldPath` = `Vec<String>`, array-only)
- **Severity:** low
- **Issue:** The same conceptual field reference has two wire grammars: `{"op":"eq","field":"id"}` works, but `{"type":"field","path":"id"}` fails — it must be `{"path":["id"]}`. Deserialize-from-string is only wired into `Filter`.
- **Failure scenario:** Client authors (or the TS SDK) naturally reuse the string shorthand from WHERE in projections and get opaque "invalid type: string, expected a sequence" errors.
- **Suggested fix:** Reuse `de_field_path` on every `FieldPath` wire field (serialization already always emits the canonical array).

### 14. `InsertOp` carries two parallel record channels with unspecified result ordering
- **File:** `crates/shamir-query-types/src/write/types.rs:81-93` (`values` + `records_idmsgpack`, "both may be present in one op (different records)")
- **Severity:** low
- **Issue:** The DTO does not define the interleaving/order of returned rows when both channels are non-empty (which channel's rows come first in `WriteResult.records` / how `QueryResult::versions` aligns), leaving the pass-through v2 path's client-visible contract implicit.
- **Suggested fix:** Document the canonical order (e.g. `values` rows then `records_idmsgpack` rows) in the field docs, or make the channels mutually exclusive at the DTO level once v2 clients settle.

### 15. `query_version` negotiation coverage is inconsistent within `DbRequest`
- **File:** `wire/db_message.rs:39-47,89-116,222-234` (Execute/TxBegin/TxExecute/CreateCursor carry it) vs `:119-132,57-81,188-211` (TxCommit/TxRollback/CreateScramUser/SetSuperuser/SetReplicator don't)
- **Severity:** low
- **Issue:** A future v3 client talking to a v2 server is rejected on `Execute` but its `TxCommit`/admin ops are accepted unversioned — the gate covers only part of the surface that shares `BatchRequest`/behavior semantics.
- **Suggested fix:** Either add `query_version` (with the same `default`) to the remaining stateful ops, or document why only batch-carrying ops need it.

### 16. "Always required" HMAC fields are `Option<String>` — required-ness exists only in prose and runtime gates
- **File:** `wire/db_message.rs:77-80,193-196,208-210` ("always required (unconditional)" over `hmac: Option<String>`); same shape throughout `auth/types.rs`, `admin/types/*`
- **Severity:** low
- **Issue:** The documented-unconditional gates (`CreateScramUser`, `SetSuperuser`, `SetReplicator`) are unenforceable at the type level; every consumer must re-implement the "None ⇒ `hmac_required`" check, and the wire can never express "this field must be present". (Accepted-verbatim rationale exists for round-tripping — `DropUserOp`'s "Option purely to allow types to roundtrip uncheckedly" — but it's applied uniformly, including where no round-trip need exists.)
- **Suggested fix:** Keep `Option` for backwards wire compat, but add `#[must_use]`-style constructor helpers or a `validate_hmacs()` method on the ops that centralizes the required-ness the docs describe.

### 17. Doc/wire mismatches and stale references
- **File:** `wire/db_message.rs:330-332` (error-code list names `fk_restrict` twice); `hmac.rs:55-68` (canonical-inputs markdown table split in two by the explanatory paragraphs — the second half renders header-less); `admin/types/validator_ops.rs:57-62,78` (examples show nested `"table": {"db":…,"repo":…,"table":…}` objects; the structs take flat `db`/`repo`/`table: String`); `batch/batch_limits.rs:9-18` (doc table lists 5 defaults, struct has 6 — `max_nesting_depth` missing); `read/limit.rs:320` (comment references `has_next_hint`; the method is `with_has_next`); `wire/mod.rs:19-20` (`CURRENT_QUERY_LANG_VERSION` re-exported at the `wire` root, `CURRENT_REPL_PROTO_VER` only reachable via `wire::repl::`)
- **Severity:** nit
- **Issue:** Protocol-reference documentation that drifts from the structs (wrong examples, split tables, duplicate vocabulary entries) is what hand-rolled clients are built from.
- **Suggested fix:** Fix each in place; re-export `CURRENT_REPL_PROTO_VER` beside `CURRENT_QUERY_LANG_VERSION`.

### 18. Inline `#[cfg(test)] mod tests` in implementation files, despite the documented `tests/` layout and existing sibling test files
- **File:** `crates/shamir-query-types/src/read/query_record.rs:302-434`; `src/write/inserted_record.rs:134-214`
- **Severity:** low
- **Issue:** CLAUDE.md's test-organisation rule 5 ("Never embed `#[cfg(test)] mod tests { ... }` inline … Move them to the `tests/` directory") — and both modules *have* `tests/` directories with matching files (`src/read/tests/query_record_tests.rs`, `src/write/tests/inserted_record_tests.rs`), so the inline copies are pure drift (these particular tests are wire round-trip tests, i.e. squarely this crate's protocol contract).
- **Suggested fix:** Move the inline tests into the existing `tests/` files.

</details>
