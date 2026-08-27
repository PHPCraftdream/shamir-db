# shamir-engine -- API & wire-protocol design

## Summary

The production wire path (BatchOp → serde-derived DTOs in `shamir-query-types`, produced by `shamir-query-builder`) is well designed — tagged enums, `skip_serializing_if` wire stability, and a fail-closed version byte on the DDL op log. Builder-only query construction is genuinely enforced: zero `serde_json` in `src/`, and ~40 test files import `shamir-query_builder`. The two real weaknesses are (1) a parallel hand-written parser family (`query_from_value` / `pagination_from_value` / `filter_from_value`, exported as public API) that parses a **different, legacy wire dialect** than the serde path and silently drops `temporal`/`with_version`/`explain`/keyset pagination — a canonical-shaped query fed to it becomes an *unbounded* read; and (2) inconsistent application of the crate's own `MetaEnvelope` versioning convention: `MemBufferConfig`, the migration `ShadowEntry`, and the index2 drop tombstone persist as raw unversioned `bincode` 1.x blobs whose shape has already churned once.

## Findings

### 1. Exported hand-written query parser speaks a dead wire dialect and silently drops query semantics
- **File:** `crates/shamir-engine/src/query/read/parser.rs:14-78`, `crates/shamir-engine/src/query/common/parser.rs:576-612` (re-exported at `query/read/mod.rs:19`, `query/mod.rs:13`)
- **Severity:** high
- **Issue:** Two parsers exist for the same logical message. The canonical one (`BatchOp::Read` → `qv_to::<ReadQuery>` via serde, `shamir-query-types/src/batch/batch_op.rs:288`) accepts pagination as the internally-tagged nested object `{"pagination": {"mode": "LimitOffset"|"Page"|"After", ...}}` (builder output, pinned by `shamir-query-builder/src/query/tests/query_tests.rs:806-1027`) plus `temporal` / `with_version` / `explain`. The engine's public `query_from_value` instead reads a top-level `"limit"` key, parses an *untagged* `{"page"|"page_size"|"limit"|"offset"}` shape (pinned by `query/read/tests/pagination_tests.rs:211-238`), cannot represent keyset `After` pagination at all, and hardcodes `temporal: Latest, with_version: false, explain: false` (parser.rs:74-76).
- **Failure scenario:** Any caller that routes a builder/serde-shaped `ReadQuery` payload through this exported function gets `map.get("limit") == None` → `Pagination::None` → the `.limit(20)` query returns the **entire table**; an `as_of` temporal read silently becomes a `Latest` read; `with_version`/`explain` are silently ignored. In-workspace only tests call it today, which is exactly why the drift was never caught — it is a public-API trap for the next consumer (SDK, FFI, tooling).
- **Suggested fix:** Either delete the parser family, or reduce `query_from_value` to a thin serde round-trip (`rmp_serde` QueryValue → `ReadQuery`, same as `BatchOp`'s `qv_to`) so there is exactly one wire grammar; failing that, `#[doc(hidden)]` + `#[deprecated]` it and fix `pagination_from_value` to accept the tagged `pagination` object. Add a differential test asserting builder-serialized queries round-trip through it.

### 2. `pagination_from_value` coerces invalid wire input instead of rejecting it
- **File:** `crates/shamir-engine/src/query/common/parser.rs:576-606`
- **Severity:** medium (high if the function stays reachable per finding 1)
- **Issue:** Four separate lenient coercions: (a) `Some(Value::Str(_))` for `limit` falls to `_ => None` — a string `"10"` silently means **no limit**; (b) negative ints are cast unchecked: `{"limit": -1}` → `(-1i64) as u64` = `u64::MAX` (same for `offset` and `page`); (c) a non-Int `page` (e.g. `"2"`) fails the `if let` and silently falls through to the limit/offset branch, dropping pagination entirely; (d) when `page` is present, `page_size` is required but the error mislabels the field as `limit.page_size` (line 583).
- **Failure scenario:** A client sending `{"limit": "10"}` or `{"limit": -1}` (both plausible from dynamic TS) receives an unbounded result set instead of a parse error — a correctness *and* DoS-amplification hazard, in the same module that exists to validate the wire.
- **Suggested fix:** Type-mismatch → `InvalidType("limit", "integer")`; `i < 0` → `InvalidField("limit", "non-negative")` (likewise offset/page/page_size); non-Int `page` → error rather than fallthrough; fix the error label to `pagination.page_size`.

### 3. `MetaEnvelope` convention not applied to three persisted bincode blobs (no version dispatch possible)
- **File:** `crates/shamir-engine/src/table/buffer_config.rs:33,47`; `crates/shamir-engine/src/migration/shadow_log.rs:79,97,114`; `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:1388-1393`
- **Severity:** medium
- **Issue:** `meta/envelope.rs` documents that *"every persisted `__meta__/*` payload"* is wrapped in the versioned `MetaEnvelope` (`magic=SDB2, version u16`), and `recovery_marker.rs` / `validator/persistence.rs` honor it. But: `MemBufferConfig` (a `MetaKey::BufferConfig` payload, written by DDL today) is raw `bincode::serialize`; `ShadowEntry` (crash-recovery-critical — `recover()` reads it on open) is raw bincode; the index2 drop tombstone `Vec<(u32, String, Option<String>)>` is raw bincode under `RecordId::system("_m.idx.drop")`. `bincode` 1.x is neither self-describing nor versioned, and the tombstone tuple has already changed shape once (#1051 added the `Option<String>` op_id) — the class of churn is real, and each change is a silent old-file hard-failure (`Codec` error) with no dispatch point. Contrast `table/ddl_op_log.rs:34,90-95`, which does this right (explicit version byte, fail-closed on unknown version).
- **Failure scenario:** `MemBufferConfig` gains a sixth knob (the struct's own doc lists five tunables and calls them evolvable) → after upgrade, every table with a persisted config fails `buffer_config::load` → `TableManager::create` errors on open; there is no version byte to migrate on.
- **Suggested fix:** Route all three through `MetaEnvelope` (the key space is already reserved), or at minimum prepend the `DDL_OP_LOG_VERSION`-style version byte + migration shim in each reader, as `ddl_op_log` already demonstrates.

### 4. `order_by` parser silently swallows invalid `nulls` values while `order` errors strictly
- **File:** `crates/shamir-engine/src/query/common/parser.rs:536-544`
- **Severity:** low
- **Issue:** An unrecognized `order` string errors (`InvalidField("order", "asc or desc")`), but an unrecognized `nulls` string (`"middle"`, typo'd `"fist"`) maps to `_ => None` — silently "no placement preference". The serde side models this as a `NullsOrder` enum, so the hand parser is strictly weaker than the canonical grammar for the same field.
- **Failure scenario:** Client typos the nulls placement; rows come back in a different order than requested with a 200-OK response; the bug surfaces as an application-level sort mystery, not a parse error.
- **Suggested fix:** Return `InvalidField("nulls", "first or last")` for unrecognized strings.

### 5. `filter_stream_tests.rs` constructs filters from raw wire maps, not the builder
- **File:** `crates/shamir-engine/src/table/tests/filter_stream_tests.rs:81` (and ~30 more `filter_from_value(&mpack!({...}))` sites in the same file)
- **Severity:** low
- **Issue:** `Cargo.toml:113-115` states the project rule — *"Tests build queries via the typed query builder instead of raw wire values"* — and sibling evaluation tests (`write_exec_tests.rs`, `fk_*_tests.rs`, `doctor_tests.rs`) do use `shamir_query_builder::filter`. `filter_stream_tests` is a filter-*evaluation* suite (its subject is streaming eval, not the wire format), so it doesn't fall under the documented serde-round-trip exception; it uses the legacy parser as a convenience constructor, which also keeps finding 1's dead dialect alive as if it were a supported input path.
- **Suggested fix:** Migrate the file to `shamir_query_builder::filter::*`; keep raw-`mpack!` construction only in files whose subject is the parser itself (`parser_tests.rs`, `query_tests.rs`).

### 6. Validator-result decoder is strict on `code` but silently lenient on `stop`
- **File:** `crates/shamir-engine/src/validator/decode.rs:59-63`
- **Severity:** low
- **Issue:** A non-string `"code"` errors (`NonStringCode`), but a non-bool `"stop"` (e.g. `"stop": "yes"` from a WASM guest) silently becomes `false`. The validator's intent to halt the chain is lost and later validators still run — the write may be accepted on a different basis than the author intended. This is an ABI-convention boundary, so leniency here is a correctness hazard, not convenience.
- **Suggested fix:** Add a `BadStopType` variant and error, mirroring `NonStringCode`.

### 7. `ShadowKey`/`MigrationShadowLog` public constructors don't enforce the documented id constraint
- **File:** `crates/shamir-engine/src/migration/shadow_key.rs:6-8,53-59`; `migration/shadow_log.rs:38-46`
- **Severity:** nit
- **Issue:** The key codec's layout (`__shadow_<id>_<lsn_be>`) is documented as safe only because *"migration_ids are UUIDs or short ASCII identifiers"*, but production ids are `format!("mig_{table}_{ns}_{rand}", ...)` (`shamir-db/src/shamir_db/execute/admin_migration.rs:88`) — they embed a user-controlled table name and contain `_`. `parse_lsn` never validates the prefix shape, so a `_`-bearing id whose bytes prefix another migration's id would make one migration's `scan_prefix` match (and `purge` delete) another's entries. Today's trailing `{:08x}` random suffix makes an actual prefix collision practically impossible, but the invariant is load-bearing and unenforced at the only place that could enforce it.
- **Suggested fix:** Validate the id charset (e.g. reject `b'_'`) in `ShadowKey::new` / `MigrationShadowLog::new`, or length-prefix the id in the key layout so the constraint disappears.

### 8. `repo/group_commit/mod.rs` contains full implementation logic
- **File:** `crates/shamir-engine/src/repo/group_commit/mod.rs:1-125`
- **Severity:** nit
- **Issue:** CLAUDE.md: "*`mod.rs` files contain re-exports only; logic lives in sibling files*". `GroupCommit` (struct + leader loop + `recv`) is implemented entirely in the `mod.rs`; every other module in this crate follows the rule (spot-checked: `query/*`, `meta/*`, `validator/mod.rs`, `table/mod.rs` are re-export-only).
- **Suggested fix:** Move `GroupCommit` to `group_commit.rs` (sibling) with `tests/` alongside; keep `mod.rs` as the manifest.
