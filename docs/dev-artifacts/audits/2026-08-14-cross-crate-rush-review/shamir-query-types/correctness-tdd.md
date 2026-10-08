<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-types — correctness-tdd independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Classification, identifier, accessor and arithmetic mechanisms persist. Local coverage ownership gaps are real but neighboring registered tests reduce severity. The TableRef acceptance claim remains refuted by exact decoder behavior.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 1 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `BatchOp::ForEach` missing from `is_admin()` while `Batch` is included — gate-bypass-shaped classification asymmetry, unpinned by any test

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A permitted non-superuser admin operation wrapped in ForEach skips the outer coarse policy while equivalent Batch is blocked. Actor propagation and per-admin DAC still prevent unrestricted escalation. A classification test must assert the chosen policy, not infer authorization safety.

Evidence: [crates/shamir-query-types/src/batch/batch_op.rs:577](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_op.rs#L577); [crates/shamir-server/src/db_handler/handler.rs:512](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L512); [crates/shamir-server/src/db_handler/tx_handlers.rs:109](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L109); [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:95](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L95).

<a id="review-2"></a>

### Claim 2 — `QueryResult::op_id` / `DdlOpStatus.op_id` / `request_id` serialize `RecordId` as raw `bin`, contradicting the crate's own base58-string wire convention and the `String`-typed poll request

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

All four fields use RecordId serialization, which emits bytes. GetDdlOpStatus and TS expect strings. ddl_tests only compares Rust-decoded fields, so both halves sharing binary encoding masks the mismatch.

Evidence: [crates/shamir-query-types/src/read/query_result.rs:190](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_result.rs#L190); [crates/shamir-query-types/src/read/ddl.rs:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/ddl.rs#L14); [crates/shamir-query-types/src/admin/types/index_ops.rs:125](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/admin/types/index_ops.rs#L125); [crates/shamir-query-types/src/admin/types/index_ops.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/admin/types/index_ops.rs#L154); [crates/shamir-types/src/types/record_id.rs:158](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/record_id.rs#L158); [crates/shamir-query-types/src/read/tests/ddl_tests.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/tests/ddl_tests.rs#L61).

<a id="review-3"></a>

### Claim 3 — `InsertedRecord` deserialization never restores `id`, and `get_value_owned("_id")` ignores the `_id` entry in `fields` — data silently inaccessible after a round-trip

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A serialized id-bearing map decodes with id=None; get_value_owned("_id") then returns None although fields contains the string. The registered roundtrip checks name/qty, not this postcondition.

Evidence: [crates/shamir-query-types/src/write/inserted_record.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/write/inserted_record.rs#L98); [crates/shamir-query-types/src/write/inserted_record.rs:114](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/write/inserted_record.rs#L114); [crates/shamir-query-types/src/write/inserted_record.rs:157](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/write/inserted_record.rs#L157).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — Planner error paths, `PaginationInfo::compute`, `QueryReference::parse`, and `collect_required_access` are owned here but tested only in other crates

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Local manifests lack the named pure-function cases. Neighbor planner tests assert specific errors; parser tests cover six errors but not UnexpectedChar. Engine lib registration is intact; facade enforcement is an integration target requiring full selection.

Evidence: [crates/shamir-query-types/src/batch/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/tests/mod.rs#L1); [crates/shamir-query-types/src/read/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/tests/mod.rs#L1); [crates/shamir-engine/src/query/batch/tests/planner_tests.rs:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/tests/planner_tests.rs#L106); [crates/shamir-engine/src/query/batch/tests/reference_tests.rs:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/tests/reference_tests.rs#L126); [crates/shamir-db/tests/enforcement_dml_e2e.rs:395](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/enforcement_dml_e2e.rs#L395); [scripts/test.sh:177](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/scripts/test.sh#L177).

<a id="review-5"></a>

### Claim 5 — Vacuous test: `fts_default_mode_is_and` asserts a value the test itself supplies

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The test supplies mode="and" and examines that value. Deleting default_fts_mode or changing it to "or" cannot affect this oracle. Decode a payload omitting mode to pin defaulting.

Evidence: [crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs#L29); [crates/shamir-query-types/src/filter/filter_enum.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_enum.rs#L162); [crates/shamir-query-types/src/filter/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/tests/mod.rs#L1).

<a id="review-6"></a>

### Claim 6 — `Pagination::resolve` multiplication can overflow; `page: 0` silently behaves as page 1 while `current_page` echoes 0

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

For page=MAX and page_size=MAX, multiplication wraps to skip=2 without checks; addition can independently wrap. Page zero produces skip zero but current_page zero. Registered engine zero-resolve coverage does not assert metadata consistency or overflow.

Evidence: [crates/shamir-query-types/src/read/limit.rs:180](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/limit.rs#L180); [crates/shamir-query-types/src/read/limit.rs:294](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/limit.rs#L294); [crates/shamir-query-types/src/read/limit.rs:296](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/limit.rs#L296); [crates/shamir-engine/src/query/read/tests/pagination_tests.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/tests/pagination_tests.rs#L55).

<a id="review-7"></a>

### Claim 7 — Inline `#[cfg(test)] mod tests` blocks violate the crate's own test-layout convention

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both inline modules are reachable under cfg(test), and sibling suites are wired. The explicit no-inline-tests rule remains violated, without lost execution coverage.

Evidence: [crates/shamir-query-types/src/read/query_record.rs:303](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L303); [crates/shamir-query-types/src/write/inserted_record.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/write/inserted_record.rs#L135); [CLAUDE.md:598](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L598).

Grouping/duplicate: [style-claude-md.md#2](style-claude-md.md#review-2). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — Four newest `hmac::canonical_*` helpers have zero tests; `create_scram_user` doc shows a trailing `\0` the implementation does not emit

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The local HMAC imports/tests omit create_function, set_superuser, set_replicator and create_scram_user. Byte layouts are therefore not independently pinned there. The existing table shows a trailing separator not emitted by join_null.

Evidence: [crates/shamir-query-types/src/tests/hmac_tests.rs:2](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/tests/hmac_tests.rs#L2); [crates/shamir-query-types/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/tests/mod.rs#L1); [crates/shamir-query-types/src/hmac.rs:68](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/hmac.rs#L68); [crates/shamir-query-types/src/hmac.rs:402](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/hmac.rs#L402).

<a id="review-9"></a>

### Claim 9 — `check_filter_depth` boundary (exactly `MAX_FILTER_DEPTH` passes, +1 fails) is not pinned

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Current rejection trees exceed the cap substantially. Starting with Eq plus a literal gives depth two: 62 Not wrappers reach 64 and 63 reach 65. Replacing &gt; with &gt;= tightens acceptance; it does not admit another level.

Evidence: [crates/shamir-query-types/src/filter/filter_enum.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_enum.rs#L240); [crates/shamir-query-types/src/filter/filter_enum.rs:242](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_enum.rs#L242); [crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs:213](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs#L213).

<a id="review-10"></a>

### Claim 10 — `TableRef` deserialization silently accepts arrays longer than 2 elements

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

A three-element input leaves one element after visit_seq. rmp-serde 1.3.1 any_inner returns LengthMismatch; serde 1.0.228 Content visitors also reject leftovers. No later source fix is required for this claimed acceptance. [Pinned decoder](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs).

Evidence: [crates/shamir-query-types/src/table_ref.rs:71](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/table_ref.rs#L71); [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949); [Cargo.lock:3204](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3204).

## Evidence and recipe corrections

- The module-local coverage deficit is Low ownership/oracle debt, not proof the functions lack behavioral tests throughout the workspace.
- The no-newest-helper-tests assertion is local byte-layout coverage, not proof that their handlers have never been exercised elsewhere.
- A second registered test, field_accepts_bare_string_as_single_segment, also fails to exercise its named wire input; it is listed separately as a new oracle finding.
- Historical Display roundtrip recipes must allow .length to canonicalize to .count rather than require textual identity.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-types -- Correctness & TDD-coverage

## Summary

The crate's serde/wire DTOs and the dependency-extraction half of the planner are well tested (real regression tests for #642/#651/#663/#660/#983, byte-level wire assertions, boundary tests), but two genuine correctness defects stand out: `BatchOp::ForEach` is omitted from `is_admin()` while its self-declared "structural sibling" `Batch` is included (feeding a superuser gate in `shamir-server` that iterates top-level entries only), and `QueryResult::op_id`/`DdlOpStatus::op_id` use `RecordId`'s raw-bin serde, contradicting the crate's own documented base58-string convention and the `String`-typed poll request. A second systemic pattern is tests stranded in other crates: the planner's cycle/depth/unknown-alias paths, `PaginationInfo::compute`, `QueryReference::parse`, and `collect_required_access` are owned here but tested only in `shamir-engine`/`shamir-db`, against CLAUDE.md's per-module `tests/` layout. One test (`fts_default_mode_is_and`) is outright vacuous.

## Findings

### 1. `BatchOp::ForEach` missing from `is_admin()` while `Batch` is included — gate-bypass-shaped classification asymmetry, unpinned by any test
- **File:** crates/shamir-query-types/src/batch/batch_op.rs:634 (is_admin list includes `BatchOp::Batch(_)` but not `BatchOp::ForEach`); contrast batch_op.rs:764-771 where `is_write()` deliberately treats the two identically ("Recursive, identical to Batch(sub)"), and for_each_op.rs:12-18 which declares ForEach a "sibling of SubBatchOp".
- **Severity:** high
- **Issue:** `is_admin()` is an exhaustive-classification function whose only consumers for control flow are the server's coarse superuser gates (`shamir-server/src/db_handler/handler.rs:512-521`, `tx_handlers.rs:109`), which iterate **top-level** `batch.queries` entries with no recursion into nested bodies. `Batch` is admin ⇒ any top-level sub-batch is blocked for non-superusers regardless of body. `ForEach` is not admin ⇒ a top-level `ForEach` whose body contains `DropDb`/`GrantRole`/`CreateUser`/… passes the coarse gate for a non-superuser session; the inner op then dispatches through the engine's `AdminExecutor` (query_runner.rs:950) with no evidence of a superuser re-check (the per-op `required_access` loop returns `None` for both container variants, and `check_destructive_hmacs` is explicitly "not an authentication gate"). The safety argument recorded at shamir-server/src/db_handler/admin.rs:590-596 ("`BatchOp::Batch` … MUST NEVER be added here") reasons only about `Batch` and never mentions `ForEach`, which has the identical `required_access == None` shape.
- **Failure scenario:** Non-superuser session sends `{"for_each": {...body: {q1: {drop_db: "prod"}}}, "over": [1], "bind_row": "x"}`. Top-level gate: `ForEach.is_admin() == false` → no `permission_denied`; `is_write()` is true but that only matters on read-only replicas; HMAC is a "did-you-mean-it" tag the session holder can always compute. The destructive DDL executes unless some deeper engine path re-checks superuser — nothing in this crate or the gate documents one.
- **Suggested fix:** Add `| BatchOp::ForEach(_)` to `is_admin()` (or make `is_admin` recurse like `is_write` does: admin if any body op is admin), and add a Red test first: `for_each_is_admin_reflects_body` mirroring `nested_batch_is_admin` (batch_types_tests.rs:578) and `for_each_is_write_reflects_body` (for_each_op_tests.rs:59). Coordinate with shamir-server (the pure-read-sub-batch over-restriction for `Batch` can be revisited separately).

### 2. `QueryResult::op_id` / `DdlOpStatus.op_id` / `request_id` serialize `RecordId` as raw `bin`, contradicting the crate's own base58-string wire convention and the `String`-typed poll request
- **File:** crates/shamir-query-types/src/read/query_result.rs:189-190 (`op_id: Option<RecordId>`, derived serde), read/ddl.rs:14 (`DdlOpStatus.op_id: RecordId`), admin/types/index_ops.rs:125 & 154 (`request_id: Option<RecordId>`); contrast read/query_result.rs:74-110 (`CorruptRecordRef.id` base58 via `id_as_base58_string`, doc: "the SAME convention every other RecordId uses on the wire … NOT raw msgpack bytes"), read/limit.rs:142-167 (`after_id` base58), and wire/db_message.rs:267-276 (`DbRequest::GetDdlOpStatus { op_id: String }`).
- **Severity:** medium
- **Issue:** `RecordId`'s own `Serialize` emits `serialize_bytes` (shamir-types/src/types/record_id.rs:158-165). Three newer DTO fields use it raw, while the crate's documented convention (and every client-facing id: `InsertedRecord._id`, `CorruptRecordRef.id`, `Pagination::after_id`) is base58 string. The DDL poll round-trip is therefore asymmetric inside this one crate: `QueryResult.op_id` arrives as 16 raw bytes (a `Uint8Array` for the TS client), but `DbRequest::GetDdlOpStatus.op_id` expects a base58 `String`, and `DdlOpStatus.op_id` in the reply is bytes again. `CorruptRecordRef`'s doc claim that base58 is universal is now false. In-crate tests only assert Rust-to-Rust round-trips (ddl_tests.rs:61-89), which mask the asymmetry.
- **Failure scenario:** A non-Rust client (or a generic `QueryValue` intermediary) captures `result.op_id` and echoes it into `GetDdlOpStatus`; the string/bytes mismatch either fails deserialization or polls for the wrong id, so crash-recovery status is unobtainable — the exact workflow the field was added for (#1015).
- **Suggested fix:** Reuse the `id_as_base58_string` `with`-module (or make it a shared helper) for `QueryResult::op_id`, `DdlOpStatus.op_id`, and the two `request_id` fields; add wire-shape tests asserting `QueryValue::Str` (mirroring `corrupt_record_ref_id_is_msgpack_string_not_bytes`). If the raw-bin shape is already deployed and frozen, document the exception and where the client converts.

### 3. `InsertedRecord` deserialization never restores `id`, and `get_value_owned("_id")` ignores the `_id` entry in `fields` — data silently inaccessible after a round-trip
- **File:** crates/shamir-query-types/src/write/inserted_record.rs:95-99 (visitor returns `InsertedRecord { id: None, fields }` — the wire `_id` stays buried in `fields`), 114-119 (`get_value_owned` early-returns on `"_id"` from `self.id` only, never falling back to `self.fields.get("_id")`).
- **Severity:** medium
- **Issue:** Serialize injects `_id` from `id`; deserialize never extracts it back. The asymmetry is acknowledged in the round-trip test (inserted_record.rs:161-163, "After round-trip, _id is stored in fields"), but the accessor was not adapted: after deserializing a `WriteResult` from the wire, `get_value_owned("_id")` returns `None` even though `_id` (as a base58 `Str`) is present in the `fields` map. No test asserts `_id` access post-round-trip — precisely the path that is broken.
- **Failure scenario:** Client deserializes a `WriteResult` and calls `records[0].get_value_owned("_id").expect("id")` to chain a follow-up write → `None` → panic/lost id, despite the id being right there in `fields`.
- **Suggested fix:** In `get_value_owned`, fall back to `self.fields.get(key).cloned()` when `key == "_id"` and `self.id` is `None` (or extract `_id` into `id` during `visit_map`); add the missing Red test first: round-trip then `get_value_owned("_id") == Some(Str(base58))`.

### 4. Planner error paths, `PaginationInfo::compute`, `QueryReference::parse`, and `collect_required_access` are owned here but tested only in other crates
- **File:** crates/shamir-query-types/src/batch/planner.rs:665 (`detect_cycle`/`CircularDependency`), :721 (`calculate_max_depth`/`TooDeep`), :229-235 (`UnknownAlias`), :803-861 (`topological_sort` insertion-order tie-breaking); read/limit.rs:287-333 (`PaginationInfo::compute`); batch/reference.rs:118-208 (`QueryReference::parse`); batch/query_entry.rs:127-155 (`collect_required_access`). Their tests live in crates/shamir-engine/src/query/batch/tests/planner_tests.rs:121/186/232/666, crates/shamir-engine/src/query/read/tests/pagination_tests.rs:71-180, crates/shamir-engine/src/query/batch/tests/reference_tests.rs, and crates/shamir-db/tests/enforcement_dml_e2e.rs:331.
- **Severity:** medium
- **Issue:** lib.rs documents that `batch::planner`/`batch::reference` were "lifted in here from shamir-engine once it became clear they only consume DTOs" — the code moved but the tests for its most load-bearing invariants stayed behind. CLAUDE.md's test organisation section mandates one `tests/` directory per module (this crate otherwise follows it meticulously), and the Red/Green/Refactor discipline assumes the failing test lives beside the code it pins. `./scripts/test.sh -p shamir-query-types` (and `@types`-style scopes) gives no signal for these functions; coverage survives only via a transitive crate's suite. The same-crate contrast is stark: `distinct_repos` — `collect_required_access`'s twin — has a dedicated 4-test file (distinct_repos_tests.rs), while the authz walk has none.
- **Suggested fix:** Port the engine's planner error-path tests, `PaginationInfo::compute` table tests, `reference_tests.rs`, and a `collect_required_access` mirror of `distinct_repos_tests.rs` into this crate's `tests/` dirs (they exercise pure functions; no engine dependency needed). Reference-parser specifics worth pinning locally: `.count`/`.length` reservation, `Chain` composition, `UnexpectedChar`/`TrailingDot`/`InvalidIndex` errors, and `Display` round-trip.

### 5. Vacuous test: `fts_default_mode_is_and` asserts a value the test itself supplies
- **File:** crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs:29-39; the behavior it names lives at filter/filter_enum.rs:162 (`#[serde(default = "default_fts_mode")]`) and :240-242.
- **Severity:** medium
- **Issue:** The test constructs `Filter::Fts { mode: "and".to_string(), .. }` and then asserts `mode == "and"` — it cannot fail and exercises neither `default_fts_mode` nor serde's defaulting machinery. No test anywhere builds a wire payload for `Fts` that omits `mode` (grep: `default_fts_mode` is referenced only by its definition), so the documented old-client fallback ("mode: 'and' (default)") is entirely unpinned — exactly the class of silent-default regression the sibling `vector_similarity_back_compat_old_payload_without_ef_fields` test (filter_enum_tests.rs:74-107) exists to prevent.
- **Suggested fix:** Replace with a wire-level test: deserialize `mpack!({"op": "fts", "field": "body", "query": "x"})` (no `mode` key) and assert `mode == "and"`; delete or fold the current vacuous body into `fts_serde_round_trip`.

### 6. `Pagination::resolve` multiplication can overflow; `page: 0` silently behaves as page 1 while `current_page` echoes 0
- **File:** crates/shamir-query-types/src/read/limit.rs:180 (`let skip = page.saturating_sub(1) * page_size;` — the `sub` is saturating, the `*` is not); limit.rs:295-298 (`current_page` echoes the raw `page`).
- **Severity:** low
- **Issue:** `page` and `page_size` are client-supplied `u64`s. `page = u64::MAX, page_size = 2` overflows the multiplication: panic in debug builds (DoS vector on a debug server), wrap in release (nonsensical `skip`/`has_prev`, wrong `PaginationInfo`). Separately, `page` is documented 1-based but never validated: `page: 0` computes `skip = 0` (identical to page 1) while `PaginationInfo::current_page` reports `Some(0)` — inconsistent metadata. No test covers either edge (the compute tests live in shamir-engine and use well-formed pages).
- **Suggested fix:** Use `page.saturating_sub(1).saturating_mul(page_size)`, and either reject `page == 0` at deserialization/validation time or normalize it to 1; pin both edges with tests in this crate's `read/tests/`.

### 7. Inline `#[cfg(test)] mod tests` blocks violate the crate's own test-layout convention
- **File:** crates/shamir-query-types/src/read/query_record.rs:302-434; crates/shamir-query-types/src/write/inserted_record.rs:134-214.
- **Severity:** low
- **Issue:** CLAUDE.md's test-organisation rule 5: "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." Both modules already have sibling `tests/` dirs with sibling test files (`read/tests/query_record_tests.rs`, `write/tests/inserted_record_tests.rs`), so the inline blocks are drift, and they duplicate coverage shapes (round-trip, `_id` handling) rather than complementing them.
- **Suggested fix:** Move the five `QueryRecord` tests and four `InsertedRecord` tests into the existing per-module test files (merging where they overlap, e.g. `partial_eq_direct_vs_inserted` vs the tests-dir accessor suite).

### 8. Four newest `hmac::canonical_*` helpers have zero tests; `create_scram_user` doc shows a trailing `\0` the implementation does not emit
- **File:** crates/shamir-query-types/src/hmac.rs:357-364 (`canonical_create_function`), :376-382 (`canonical_set_superuser`), :388-394 (`canonical_set_replicator`), :402-408 (`canonical_create_scram_user`); doc drift at hmac.rs:68; tests absence in src/tests/hmac_tests.rs (every pre-existing helper has byte-level assertions there).
- **Severity:** low
- **Issue:** The module's header declares "Wire-format-stable: changing a layout here is a breaking protocol change" and "server and client … must agree byte-for-byte" — yet the four most recently added canonical inputs (backing `CreateFunction`'s conditional gate and three unconditional gates) are unpinned, and the `create_scram_user` doc-comment layout (`b"create_scram_user\0<name>\0<role1>\0...\0"` — trailing separator) disagrees with `join_null`'s output (no trailing NUL). Anyone "fixing" the implementation to match the doc, or reordering parts, would break the protocol with no failing test.
- **Suggested fix:** Add byte-equality tests for all four (mirroring `canonical_drop_*`/group-op tests), including the multi-role ordering guarantee and the empty-roles case; correct the doc's trailing-`\0` (or add it to the implementation, deliberately, before any client ships).

### 9. `check_filter_depth` boundary (exactly `MAX_FILTER_DEPTH` passes, +1 fails) is not pinned
- **File:** crates/shamir-query-types/src/filter/filter_enum.rs:219-238; test at filter/tests/filter_enum_tests.rs:213-245 uses only a 100-deep chain.
- **Severity:** nit
- **Issue:** An off-by-one relaxation (e.g. `depth >= MAX`) would keep the existing 100-deep rejection test green while re-admitting one extra level; the boundary value itself is never asserted in either direction.
- **Suggested fix:** Add a test building exactly 64 nested `Not`s (must be `Ok`) and 65 (must be `Err`).

### 10. `TableRef` deserialization silently accepts arrays longer than 2 elements
- **File:** crates/shamir-query-types/src/table_ref.rs:71-79 (`visit_seq` reads exactly two elements, never checks `seq.next_element()?` for a trailing `None`).
- **Severity:** nit
- **Issue:** `["db", "repo", "table", "extra"]` deserializes successfully as `repo="db", table="repo"`, silently discarding the tail — a mis-shaped payload becomes a *different valid-looking* target table rather than an error. No direct `TableRef` wire tests exist in this crate (only indirect coverage via `BatchOp` payloads).
- **Suggested fix:** After reading the second element, assert `seq.next_element::<serde::de::IgnoredAny>()?.is_none()` (error on extras), and add a small `table_ref` test file covering both wire forms plus the too-short/too-long rejections.

</details>
