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
