# shamir-query-types — Consolidated 7-lens review (synthesized)

Crate: `crates/shamir-query-types/` — the pure-DTO shared client/server layer for the
post-handshake msgpack wire protocol (`DbRequest`/`DbResponse`/repl), filter/query
value trees, admin/auth DTOs, the canonical HMAC intent-confirmation scheme for
destructive ops, and the batch planner lifted in from `shamir-engine`. No I/O, no
async, no concurrency primitives anywhere in the crate.

Review basis — the 7 lens reports of the 2026-08-14 cross-crate sweep, read in full:
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`api-wire-protocol.md`, `error-handling-lifecycle.md`, `performance-hotpath.md`,
`style-claude-md.md` (this directory). Structure/dedup calibrated against the two
completed exemplar syntheses: `shamir-client-node/SUMMARY.md` and
`shamir-transport-ipc/SUMMARY.md`. Workspace context taken from the sweep's
`SUMMARY.md` per-crate rows (66 lens-tagged findings, 0c/7h/21m/27l/11n — matches
the source files exactly; its counts are per-report as filed, deduped here). A
read-only synthesis pass: load-bearing file:line references were spot-checked
against the crate source (`batch_op.rs` `is_admin`, `filter_enum.rs`
`check_filter_depth`, `limit.rs:180`, `inserted_record.rs` visitor/accessor,
`planner.rs` DFS, `validator/mod.rs`/`call/mod.rs`/`filter/mod.rs`), and every
checked reference was confirmed; no build/test/lint commands were run; no source
file was modified. No new defects were found during spot-checking, so nothing below
is marked "(added during synthesis)".

## Executive summary

The crate's skeleton is among the cleanest in the sweep — per-module `tests/`
directories with ~350 tests, meticulous serde forward-compat hygiene, zero
concurrency surface — but it is not shippable as-is, and all three headline
clusters are in this crate's own wire-facing code: (1) **the unbounded-recursion
DoS** — every recursive DTO (`Filter`, `FilterValue`, `Cond`, `QueryValue`,
`BatchOp`) deserializes with no parse-time depth bound, and every post-parse guard
the crate brags about (`MAX_FILTER_DEPTH`, `NESTING_WALK_LIMIT`) structurally runs
too late, so one authenticated ~1 MB frame aborts the whole server process, with
the planner's own tree walks (`detect_cycle`/`calculate_max_depth`) providing a
second, operator-configurable bite; (2) **gate-classification asymmetry** —
`BatchOp::ForEach` is missing from `is_admin()` while its self-declared structural
sibling `Batch` is included, and the destructive-op HMAC gate never descends into
`Batch`/`ForEach` bodies, so destructive DDL can slip past the superuser coarse
gate and execute with no "did-you-mean-it" tag; (3) **silent wire coercion** —
`InsertedRecord` drops `_id` on deserialize (killing the documented client access
path) and `FilterValue` lossily coerces `uint64 > i64::MAX` to `Float`, so equality
filters silently never match. Fix the DoS cluster and the gate asymmetry before
anything else ships from this crate (P0); the `BatchOp::deserialize` triple-codec
hot path and the RecordId/base58 wire asymmetry are the top P1 items.

---

## 1. correctness-tdd

Lens summary carried forward: serde/wire DTOs and the dependency-extraction half of
the planner are well tested (real regression tests for #642/#651/#663/#660/#983,
byte-level wire assertions, boundary tests); the systemic weakness is gate
classification (`is_admin`) and tests stranded in other crates, against CLAUDE.md's
per-module `tests/` layout.

### 1.1 — high — `BatchOp::ForEach` missing from `is_admin()` while `Batch` is included — gate-bypass-shaped classification asymmetry, unpinned by any test
- File:line: `crates/shamir-query-types/src/batch/batch_op.rs:634` (is_admin list
  includes `BatchOp::Batch(_)` but not `BatchOp::ForEach` — verified in source:
  the `matches!` list at `:577-645` has no `ForEach` arm); contrast
  `batch_op.rs:764-771` where `is_write()` deliberately treats the two identically
  ("Recursive, identical to Batch(sub)"), and `for_each_op.rs:12-18` which declares
  ForEach a "sibling of SubBatchOp".
- Issue: `is_admin()` is an exhaustive-classification function whose only consumers
  for control flow are the server's coarse superuser gates
  (`shamir-server/src/db_handler/handler.rs:512-521`, `tx_handlers.rs:109`), which
  iterate **top-level** `batch.queries` entries with no recursion into nested
  bodies. `Batch` is admin ⇒ any top-level sub-batch is blocked for non-superusers
  regardless of body. `ForEach` is not admin ⇒ a top-level `ForEach` whose body
  contains `DropDb`/`GrantRole`/`CreateUser`/… passes the coarse gate for a
  non-superuser session; the inner op then dispatches through the engine's
  `AdminExecutor` (query_runner.rs:950) with no evidence of a superuser re-check
  (the per-op `required_access` loop returns `None` for both container variants,
  and `check_destructive_hmacs` is explicitly "not an authentication gate"). The
  safety argument recorded at `shamir-server/src/db_handler/admin.rs:590-596`
  ("`BatchOp::Batch` … MUST NEVER be added here") reasons only about `Batch` and
  never mentions `ForEach`, which has the identical `required_access == None` shape.
- Failure scenario: Non-superuser session sends
  `{"for_each": {...body: {q1: {drop_db: "prod"}}}, "over": [1], "bind_row": "x"}`.
  Top-level gate: `ForEach.is_admin() == false` → no `permission_denied`;
  `is_write()` is true but that only matters on read-only replicas; HMAC is a
  "did-you-mean-it" tag the session holder can always compute. The destructive DDL
  executes unless some deeper engine path re-checks superuser — nothing in this
  crate or the gate documents one.
- Suggested fix: Add `| BatchOp::ForEach(_)` to `is_admin()` (or make `is_admin`
  recurse like `is_write` does: admin if any body op is admin), and add a Red test
  first: `for_each_is_admin_reflects_body` mirroring `nested_batch_is_admin`
  (batch_types_tests.rs:578) and `for_each_is_write_reflects_body`
  (for_each_op_tests.rs:59). Coordinate with shamir-server (the
  pure-read-sub-batch over-restriction for `Batch` can be revisited separately).
- Dedup: **security-crypto 3.4** flags this same `is_admin`/`ForEach`
  misclassification (as the (a) half of its finding, rated medium there); the
  distinct nested-op HMAC non-recursion half of 3.4 is counted separately under
  security.

### 1.2 — medium — `QueryResult::op_id` / `DdlOpStatus.op_id` / `request_id` serialize `RecordId` as raw `bin`, contradicting the crate's own base58-string wire convention and the `String`-typed poll request
- File:line: `crates/shamir-query-types/src/read/query_result.rs:189-190`
  (`op_id: Option<RecordId>`, derived serde), `read/ddl.rs:14`
  (`DdlOpStatus.op_id: RecordId`), `admin/types/index_ops.rs:125 & 154`
  (`request_id: Option<RecordId>`); contrast `read/query_result.rs:74-110`
  (`CorruptRecordRef.id` base58 via `id_as_base58_string`, doc: "the SAME
  convention every other RecordId uses on the wire … NOT raw msgpack bytes"),
  `read/limit.rs:142-167` (`after_id` base58), and `wire/db_message.rs:267-276`
  (`DbRequest::GetDdlOpStatus { op_id: String }`).
- Issue: `RecordId`'s own `Serialize` emits `serialize_bytes`
  (`shamir-types/src/types/record_id.rs:158-165`). Three newer DTO fields use it
  raw, while the crate's documented convention (and every client-facing id:
  `InsertedRecord._id`, `CorruptRecordRef.id`, `Pagination::after_id`) is base58
  string. The DDL poll round-trip is therefore asymmetric inside this one crate:
  `QueryResult.op_id` arrives as 16 raw bytes (a `Uint8Array` for the TS client),
  but `DbRequest::GetDdlOpStatus.op_id` expects a base58 `String`, and
  `DdlOpStatus.op_id` in the reply is bytes again. `CorruptRecordRef`'s doc claim
  that base58 is universal is now false. In-crate tests only assert Rust-to-Rust
  round-trips (ddl_tests.rs:61-89), which mask the asymmetry.
- Failure scenario: A non-Rust client (or a generic `QueryValue` intermediary)
  captures `result.op_id` and echoes it into `GetDdlOpStatus`; the string/bytes
  mismatch either fails deserialization or polls for the wrong id, so
  crash-recovery status is unobtainable — the exact workflow the field was added
  for (#1015).
- Suggested fix: Reuse the `id_as_base58_string` `with`-module (or make it a shared
  helper) for `QueryResult::op_id`, `DdlOpStatus.op_id`, and the two `request_id`
  fields; add wire-shape tests asserting `QueryValue::Str` (mirroring
  `corrupt_record_ref_id_is_msgpack_string_not_bytes`). If the raw-bin shape is
  already deployed and frozen, document the exception and where the client
  converts.
- Dedup: **api-wire-protocol 4.4** is the same defect from the protocol-design
  angle (one `RecordId` identity, three wire forms).

### 1.3 — medium — *(dedup: primary write-up at 4.1 — same root-cause defect; api lens rated it high)* `InsertedRecord` deserialization never restores `id`, and `get_value_owned("_id")` ignores the `_id` entry in `fields`
- File:line: `crates/shamir-query-types/src/write/inserted_record.rs:95-99`
  (visitor returns `InsertedRecord { id: None, fields }` — verified in source),
  `:114-119` (`get_value_owned` early-returns on `"_id"` from `self.id` only,
  never falling back to `self.fields.get("_id")`).
- Correctness-lens additions folded into 4.1: the round-trip test itself
  acknowledges the asymmetry (inserted_record.rs:161-163, "After round-trip, _id
  is stored in fields") but the accessor was not adapted; **no test asserts `_id`
  access post-round-trip — precisely the path that is broken**; the Red test to
  add is round-trip then `get_value_owned("_id") == Some(Str(base58))`.

### 1.4 — medium — Planner error paths, `PaginationInfo::compute`, `QueryReference::parse`, and `collect_required_access` are owned here but tested only in other crates
- File:line: `crates/shamir-query-types/src/batch/planner.rs:665`
  (`detect_cycle`/`CircularDependency`), `:721` (`calculate_max_depth`/`TooDeep`),
  `:229-235` (`UnknownAlias`), `:803-861` (`topological_sort` insertion-order
  tie-breaking); `read/limit.rs:287-333` (`PaginationInfo::compute`);
  `batch/reference.rs:118-208` (`QueryReference::parse`);
  `batch/query_entry.rs:127-155` (`collect_required_access`). Their tests live in
  `crates/shamir-engine/src/query/batch/tests/planner_tests.rs:121/186/232/666`,
  `crates/shamir-engine/src/query/read/tests/pagination_tests.rs:71-180`,
  `crates/shamir-engine/src/query/batch/tests/reference_tests.rs`, and
  `crates/shamir-db/tests/enforcement_dml_e2e.rs:331`.
- Issue: lib.rs documents that `batch::planner`/`batch::reference` were "lifted in
  here from shamir-engine once it became clear they only consume DTOs" — the code
  moved but the tests for its most load-bearing invariants stayed behind.
  CLAUDE.md's test organisation section mandates one `tests/` directory per module
  (this crate otherwise follows it meticulously), and the Red/Green/Refactor
  discipline assumes the failing test lives beside the code it pins.
  `./scripts/test.sh -p shamir-query-types` (and `@types`-style scopes) gives no
  signal for these functions; coverage survives only via a transitive crate's
  suite. The same-crate contrast is stark: `distinct_repos` —
  `collect_required_access`'s twin — has a dedicated 4-test file
  (distinct_repos_tests.rs), while the authz walk has none.
- Failure scenario: a regression in `detect_cycle`, `QueryReference::parse`, or
  `PaginationInfo::compute` ships green through this crate's own test gate; only a
  downstream crate's suite (run via a different scope) can catch it — and per the
  error-handling lens (5.5, deduped here), the four headline `BatchError` variants
  documented on `BatchPlanner::plan` (`TooManyQueries`, `UnknownAlias`,
  `CircularDependency`, `TooDeep`) plus `QueryRecordVisitor::visit_f64`'s
  non-finite-float rejection (`read/query_record.rs:117-122`) are exactly the
  error paths with zero coverage under this crate's scope.
- Suggested fix: Port the engine's planner error-path tests, `PaginationInfo::compute`
  table tests, `reference_tests.rs` (happy paths + all seven `ReferenceParseError`
  variants: `MissingAt`, `EmptyAlias`, `InvalidAlias`, `UnclosedBracket`,
  `InvalidIndex`, `TrailingDot`, `UnexpectedChar`), a `collect_required_access`
  mirror of `distinct_repos_tests.rs`, and a NaN/inf `QueryRecord` decode test into
  this crate's `tests/` dirs (they exercise pure functions; no engine dependency
  needed). Reference-parser specifics worth pinning locally: `.count`/`.length`
  reservation, `Chain` composition, `UnexpectedChar`/`TrailingDot`/`InvalidIndex`
  errors, and `Display` round-trip.
- Dedup: **error-handling-lifecycle 5.5** is the same defect (its concrete gaps —
  headline planner error variants, `QueryReference::parse`, non-finite float — are
  folded into this entry).

### 1.5 — medium — Vacuous test: `fts_default_mode_is_and` asserts a value the test itself supplies
- File:line: `crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs:29-39`;
  the behavior it names lives at `filter/filter_enum.rs:162`
  (`#[serde(default = "default_fts_mode")]`) and `:240-242`.
- Issue: The test constructs `Filter::Fts { mode: "and".to_string(), .. }` and then
  asserts `mode == "and"` — it cannot fail and exercises neither
  `default_fts_mode` nor serde's defaulting machinery. No test anywhere builds a
  wire payload for `Fts` that omits `mode` (grep: `default_fts_mode` is referenced
  only by its definition), so the documented old-client fallback ("mode: 'and'
  (default)") is entirely unpinned — exactly the class of silent-default regression
  the sibling `vector_similarity_back_compat_old_payload_without_ef_fields` test
  (filter_enum_tests.rs:74-107) exists to prevent.
- Failure scenario: someone changes `default_fts_mode` to `"or"` (or drops the
  `#[serde(default)]`) and every test stays green while old clients' FTS semantics
  silently change.
- Suggested fix: Replace with a wire-level test: deserialize
  `mpack!({"op": "fts", "field": "body", "query": "x"})` (no `mode` key) and assert
  `mode == "and"`; delete or fold the current vacuous body into
  `fts_serde_round_trip`.

### 1.6 — low — `Pagination::resolve` multiplication can overflow; `page: 0` silently behaves as page 1 while `current_page` echoes 0
- File:line: `crates/shamir-query-types/src/read/limit.rs:180`
  (`let skip = page.saturating_sub(1) * page_size;` — verified in source: the `sub`
  is saturating, the `*` is not); `limit.rs:295-298` (`current_page` echoes the raw
  `page`); error-handling lens adds `limit.rs:294` (`skip + page_size < total` in
  `PaginationInfo::compute`, unchecked `+`).
- Issue: `page` and `page_size` are client-supplied `u64`s.
  `page = u64::MAX, page_size = 2` overflows the multiplication: panic in debug
  builds (DoS vector on a debug server), wrap in release (nonsensical
  `skip`/`has_prev`, wrong `PaginationInfo` — e.g. the error lens computes the
  wrap to `skip = 2`, so a client asking for `u64::MAX` gets page-2 rows).
  Separately, `page` is documented 1-based but never validated: `page: 0` computes
  `skip = 0` (identical to page 1) while `PaginationInfo::current_page` reports
  `Some(0)` — inconsistent metadata. No test covers either edge (the compute tests
  live in shamir-engine and use well-formed pages).
- Failure scenario: hostile/buggy client sends `Pagination::Page { page: u64::MAX,
  page_size: u64::MAX }`: debug-build panic at line 180 ("attempt to multiply with
  overflow"); release-build wrap into wrong `skip` and a second wrap at line 294
  into wrong `has_next`. Wrong metadata / wrong page, no corruption — but a panic
  in any debug-mode deployment and a silently wrong answer in release.
- Suggested fix: Use `page.saturating_sub(1).saturating_mul(page_size)` and
  `skip.saturating_add(page_size) < total` at line 294, and either reject
  `page == 0` at deserialization/validation time or normalize it to 1; pin both
  edges with tests in this crate's `read/tests/`.
- Dedup: **error-handling-lifecycle 5.4** is the same defect (the line-294 site is
  folded in above).

### 1.7 — low — *(dedup: primary write-up at 7.2 — style lens owns the CLAUDE.md conformance defect and rated it high)* Inline `#[cfg(test)] mod tests` blocks violate the crate's own test-layout convention
- File:line: `crates/shamir-query-types/src/read/query_record.rs:302-434`;
  `crates/shamir-query-types/src/write/inserted_record.rs:134-214`.
- Correctness-lens additions folded into 7.2: both modules already have sibling
  `tests/` dirs (`read/tests/query_record_tests.rs`,
  `write/tests/inserted_record_tests.rs`), so the inline blocks are drift that
  duplicates coverage shapes (round-trip, `_id` handling) rather than complementing
  them (e.g. `partial_eq_direct_vs_inserted` vs the tests-dir accessor suite).

### 1.8 — low — Four newest `hmac::canonical_*` helpers have zero tests; `create_scram_user` doc shows a trailing `\0` the implementation does not emit
- File:line: `crates/shamir-query-types/src/hmac.rs:357-364`
  (`canonical_create_function`), `:376-382` (`canonical_set_superuser`), `:388-394`
  (`canonical_set_replicator`), `:402-408` (`canonical_create_scram_user`); doc
  drift at `hmac.rs:68`; tests absence in `src/tests/hmac_tests.rs` (every
  pre-existing helper has byte-level assertions there).
- Issue: The module's header declares "Wire-format-stable: changing a layout here
  is a breaking protocol change" and "server and client … must agree byte-for-byte"
  — yet the four most recently added canonical inputs (backing `CreateFunction`'s
  conditional gate and three unconditional gates) are unpinned, and the
  `create_scram_user` doc-comment layout (`b"create_scram_user\0<name>\0<role1>\0...\0"`
  — trailing separator) disagrees with `join_null`'s output (no trailing NUL).
  Anyone "fixing" the implementation to match the doc, or reordering parts, would
  break the protocol with no failing test.
- Failure scenario: a well-meaning doc-driven "fix" or a refactor reorders
  canonical-input parts; every test stays green; every deployed client's tags stop
  verifying — a breaking protocol change shipped as a patch.
- Suggested fix: Add byte-equality tests for all four (mirroring
  `canonical_drop_*`/group-op tests), including the multi-role ordering guarantee
  and the empty-roles case; correct the doc's trailing-`\0` (or add it to the
  implementation, deliberately, before any client ships).

### 1.9 — nit — `check_filter_depth` boundary (exactly `MAX_FILTER_DEPTH` passes, +1 fails) is not pinned
- File:line: `crates/shamir-query-types/src/filter/filter_enum.rs:219-238` (verified
  in source); test at `filter/tests/filter_enum_tests.rs:213-245` uses only a
  100-deep chain.
- Issue: An off-by-one relaxation (e.g. `depth >= MAX`) would keep the existing
  100-deep rejection test green while re-admitting one extra level; the boundary
  value itself is never asserted in either direction.
- Failure scenario: the 64-deep contract silently becomes 65-deep (or is not
  enforced at all for the shapes 3.3 shows are already missed) with a green suite.
- Suggested fix: Add a test building exactly 64 nested `Not`s (must be `Ok`) and 65
  (must be `Err`). Related: the depth-bounding defects themselves are findings
  3.1/3.3; this is the missing boundary pin.

### 1.10 — nit — `TableRef` deserialization silently accepts arrays longer than 2 elements
- File:line: `crates/shamir-query-types/src/table_ref.rs:71-79` (`visit_seq` reads
  exactly two elements, never checks `seq.next_element()?` for a trailing `None`).
- Issue: `["db", "repo", "table", "extra"]` deserializes successfully as
  `repo="db", table="repo"`, silently discarding the tail — a mis-shaped payload
  becomes a *different valid-looking* target table rather than an error. No direct
  `TableRef` wire tests exist in this crate (only indirect coverage via `BatchOp`
  payloads); inconsistent with the crate's strict-rejection precedent
  (`de_binary_strict`, `AfterPathIgnored`).
- Failure scenario: a client emits a three-element table tuple; the op silently
  targets the wrong table instead of failing with "invalid length".
- Suggested fix: After reading the second element, assert
  `seq.next_element::<serde::de::IgnoredAny>()?.is_none()` (error on extras, e.g.
  `invalid_length(2, &"exactly 2")`), and add a small `table_ref` test file
  covering both wire forms plus the too-short/too-long rejections.
- Dedup: **error-handling-lifecycle 5.9** is the same defect.

## 2. concurrency-lockfree

**No findings — clean.** Pure-DTO crate, fully verified against the theme: every
`.rs` file under `src/` (plus `Cargo.toml` and `benches/batch_planner.rs`) was read;
zero concurrency primitives of any kind — no `std::sync::Mutex`/`RwLock`, no
`parking_lot`, no atomics, no `scc::*`/`dashmap`/`ArcSwap`, no tokio, no
threads/channels, no `static` items, no `unsafe` (grep-verified across code and
tests). No `async fn`/`.await` exists (the only `.await`-looking text is a
doc-comment example, `admin/types/schema_ops.rs:54`), so locks-across-await is
structurally impossible; the `scc::*::len()` O(N) ban is inapplicable and every
observed `.len()` is O(1). All hash-keyed structures come from `shamir_collections`
(`TMap`/`TSet`/`TFxSet`/`new_map`/`new_set`); the only `std::collections` import is
`VecDeque` (planner.rs:60). `BatchPlanner::plan` is a pure function over borrowed
DTOs; `BatchPlan::stages` is honestly documented (planner.rs:13-26) as a logical
grouping that today's executor drives sequentially, citing the oql-01 ADR — the
opposite of an overclaimed parallelism guarantee. Dependencies confirm the boundary
(serde/serde_bytes/rmp-serde/indexmap/num-bigint/hmac/sha2 + two workspace types
crates).

Considered-and-rejected as out-of-theme (recorded for sibling reviewers, not
counted): `planner.rs:758-760` doc says "iterative worklist" while
`max_nesting_depth_of_ops` actually recurses (safe — capped at `NESTING_WALK_LIMIT`
= 64 frames); `planner.rs:663` doc says `HashSet<&str>` where the code uses
`TFxSet<&str>` (Fx-compliant); the inline `#[cfg(test)]` blocks (finding 7.2);
`InsertedRecord::serialize` per-row allocation (finding 6.2).

## 3. security-crypto

Lens summary carried forward: the crypto core is sound — no `unsafe` anywhere,
constant-time tag verification (`Hmac::verify_slice`), domain-separated session-key
derivation, `SecretString` hygiene on passwords, hardened exhaustive `match`
classifiers (`required_access`, `is_write`) with no wildcard arms. The real
boundary weaknesses are on the untrusted-input side (depth bounds, HMAC coverage
and canonical-form ambiguities).

### 3.1 — high — No parse-time depth bound on recursive DTO deserialization — remote stack-overflow abort *(the headline finding)*
- File:line: `crates/shamir-query-types/src/filter/filter_enum.rs:9,127`
  (`Filter::Not(Box<Filter>>)`), `src/filter/filter_value.rs:46,70-74`
  (`Array(Vec<FilterValue>)`, `Cond(Box<Cond>)`), `src/batch/batch_op.rs:262`
  (`QueryValue::deserialize` + nested `Batch`/`ForEach` re-entry),
  `src/batch/sub_batch_op.rs:13`, `src/batch/for_each_op.rs:37`.
- Issue: `Filter`, `FilterValue`, `Cond`, `SelectExpr`, `QueryValue` and `BatchOp`
  (via `SubBatchOp`/`ForEachOp` → `BatchRequest` → `QueryEntry` → `BatchOp`) are
  all self-referential, and their serde `Deserialize` impls recurse once per
  nesting level. `rmp-serde` has no depth limiter, and
  `MAX_FILTER_DEPTH`/`check_filter_depth` (filter_enum.rs:7-9, 219) can only run
  *after* deserialization completes — the doc's stated purpose ("rejected to
  prevent stack overflow post-handshake") is unachievable for the decode itself,
  which is exactly where the overflow happens. A ~1 MB msgpack frame of nested
  arrays/maps (≈100k depth) inside an `Execute` envelope blows the thread stack
  before any DTO-level guard can run; in Rust that aborts the whole server
  process, taking down every session. (The error-handling lens notes the only
  current backstop is rmp-serde's incidental ~1024-container decode limit — an
  accident of the codec, not this crate's contract, and one that silently changes
  if the wire codec ever does.)
- Failure scenario: Any authenticated client sends
  `{"op":"execute","db":"x","batch":{"queries":{"a":{"for_each":{"over":[],"bind_row":"r","queries":{"b":{"for_each":{...}}}}}}}}`
  (or nested `[[[[...]]]]` as an insert value, or a ~10⁴–10⁵-deep chain of
  `{"batch": {"batch": …}}`/`not` wrappers — a few KB of payload). Deserialization
  recurses ~100k frames deep on a standard 2 MB tokio worker stack →
  `thread has overflowed its stack` → process abort → full-server DoS from one
  frame, restart-loopable, before `BatchLimits` is ever consulted.
- Suggested fix: Enforce depth at decode time: a depth-counting `Deserializer`
  wrapper (fails at e.g. 128 nested containers) applied wherever untrusted frames
  enter these types, or manual bounded `Deserialize` visitors for the recursive
  spine (`Filter::Not`, `FilterValue::Array/Cond`, `QueryValue::List/Map`,
  `BatchOp::Batch/ForEach`) — `BatchOp::deserialize` already buffers the whole map,
  so a cheap depth pre-pass over the buffered `QueryValue` also works. Pair it with
  a transport-level max-frame check. Update the `MAX_FILTER_DEPTH` doc to state it
  bounds *post-parse* walks only, not decode.
- Dedup: **api-wire-protocol 4.7** (depth caps are post-deserialization checks) and
  the unbounded-decode half of **performance-hotpath 6.3** are the same defect.

### 3.2 — medium — Unbounded recursive walks over already-parsed attacker trees
- File:line: `src/batch/batch_op.rs:764,771` (`is_write` recursion),
  `src/batch/query_entry.rs:102-113,140-155` (`collect_repos` /
  `collect_required_access_into`), `src/batch/planner.rs:367-442`
  (`extract_deps_from_value` over `QueryValue`), `planner.rs:445-505,535-569,584-606,618-648`
  (`extract_deps_from_filter`, `contains_field_based_comparison`,
  `filter_value_contains_field_based_comparison`, `extract_deps_from_filter_value`),
  `planner.rs:671-703,721-754` (`detect_cycle` DFS — recursive `dfs` verified in
  source at :692, `calculate_max_depth`).
- Issue: The crate already solves this class iteratively where it noticed it —
  `max_nesting_depth_of_ops` (planner.rs:763-796, `NESTING_WALK_LIMIT = 64`)
  exists precisely "so a malicious deeply-nested payload cannot blow the call
  stack", and `check_filter_depth` is likewise iterative. But every other walk over
  the same untrusted trees recurses without any bound: write-value `QueryValue`
  trees have no depth check anywhere (unlike `Filter`, whose depth is checked
  engine-side), and the `when`/`$cond` walks run at plan time on trees
  `validate_filter_depth` never inspects. The error-handling lens (5.1, deduped
  here) sharpens the planner case: `detect_cycle`'s DFS and `calculate_max_depth`'s
  `depth()` (planner.rs:735-737) recurse once per node, and recursion depth equals
  the longest dependency chain, bounded only by `limits.max_queries` — a value the
  crate itself never caps (server clamp is operator-configured
  `max_queries_per_batch`, default 100 but explicitly settable arbitrarily high;
  `QueryLimitsCap::UNLIMITED` is `usize::MAX`).
- Failure scenario: (planner variant, from 5.1) An operator configures
  `max_queries_per_batch = 500_000` (plausible for a bulk-load deployment); a
  client submits an acyclic linear chain `a1 → a2 → … → aN`; the `max_queries`
  check passes, `detect_cycle` recurses N frames deep, and the ~2 MB tokio worker
  stack overflows. A stack overflow is an abort, not a panic: no unwind, no `Drop`
  guards run, the whole server process dies — remotely triggerable,
  restart-loopable. (walk variant) A moderately deep (few-thousand-level) nested
  map as an `InsertOp.values[0]` or inside a `$cond` chain — deep enough to decode
  on a large-stack thread — overflows `extract_deps_from_value` during planning.
  Same process-abort outcome as 3.1, second bite at the apple.
- Suggested fix: Give the recursive walkers an explicit depth parameter capped at a
  small constant (nesting is already capped at 4 by default), returning
  `BatchError::NestingTooDeep`/`TooDeep` on exceed; convert `extract_deps_from_value`,
  `detect_cycle`, and `calculate_max_depth` to the same worklist shape as
  `max_nesting_depth_of_ops` (`topological_sort` is already iterative — Kahn's —
  and is the in-file pattern). One shared bounded-walk helper for
  `QueryValue`/`FilterValue` trees would prevent the next drift. Coordinate with
  performance finding 6.7 (the same walkers are run three separate times per
  request — the fused walk should be the bounded one).
- Dedup: **error-handling-lifecycle 5.1** (unbounded `detect_cycle`/
  `calculate_max_depth` recursion) is the same defect; its operator-misconfig
  scenario is folded in above.

### 3.3 — medium — `check_filter_depth` does not descend into `FilterValue` — contradicts its own `$cond` claim
- File:line: `crates/shamir-query-types/src/filter/filter_enum.rs:7-8,219-238`
  (verified in source: the match pushes children only for `And`/`Or`/`Not`;
  comparison variants hit `_ => {}`).
- Issue: The `MAX_FILTER_DEPTH` doc says "Deeply-nested `$cond`/`not`/`and`/`or`
  beyond this cap will be rejected", but `check_filter_depth`'s match only recurses
  through `And`/`Or`/`Not`; the `_ => {}` arm ignores every comparison variant's
  `FilterValue` operand. `FilterValue::Cond` embeds `Cond.condition: Box<Filter>`
  (cond.rs:44), so a 100k-deep `and/or/not` chain hidden inside a `$cond` (e.g. as
  `Eq.value`) passes the check untouched. Downstream, the engine's
  `validate_filter_depth` (shamir-engine `batch_validate.rs:78-97`) only inspects
  top-level `Read`/`Delete`/`Update` WHERE clauses — not `when`, not
  `GroupBy.having`, not nested bodies — so nothing else catches it either. The
  error-handling lens (5.2, deduped here) adds: today the only effective backstop
  for value-tree depth is rmp-serde's incidental ~1024-container decode limit, and
  any depth that survives it is re-walked unbounded by the planner
  (`extract_deps_from_filter`/`filter_value_contains_field_based_comparison`,
  planner.rs:535-606) and the evaluator.
- Failure scenario: Client sends
  `{"op":"eq","field":"x","value":{"$cond":{"if":<64+-deep and/or/not tree>,"then":1,"else":2}}}`
  (or a WHERE whose `$cond.then` embeds a ~900-deep `And` chain). Depth check
  passes; the planner's `extract_deps_from_filter_value` then recurses the full
  hidden depth (finding 3.2) and/or the evaluator recurses at run time — the
  crate's stated 64-deep guarantee is not enforced for this shape.
- Suggested fix: Extend `check_filter_depth` to walk `FilterValue` operands
  (`Cond` condition/branches, `Expr`/`FnCall` args, `Array` items — mirroring
  `extract_deps_from_filter_value`'s coverage) in the same iterative stack walk —
  the planner already has the mutually-recursive pair to mirror — or fix the doc to
  drop the `$cond` claim. Add a test with a `$cond`-embedded over-deep tree.
- Dedup: **error-handling-lifecycle 5.2** (rated low there) and the guard-coverage
  half of **performance-hotpath 6.3** are the same defect.

### 3.4 — medium — Destructive-op HMAC gate never reaches ops nested in `Batch`/`ForEach` *(the `is_admin`/`ForEach` misclassification half is deduped into 1.1; this entry's distinct defect is the nested-op HMAC non-recursion)*
- File:line: `crates/shamir-query-types/src/batch/batch_op.rs:577-646` (`is_admin` —
  includes `BatchOp::Batch(_)` at :634 but not `ForEach(_)`, `Call(_)`,
  `Subscribe`/`Unsubscribe`), cf. `is_write`'s deliberately exhaustive match at
  `:660-773` and `required_access`'s no-wildcard rationale at `:449-456`.
- Issue: (a) *(deduped into 1.1 — same root-cause defect, rated high there)*
  `is_admin` is a `matches!` list that silently defaults new/unlisted variants to
  `false` — the exact "wildcard silently swallows the decision" weakness the
  crate's own `is_write`/`required_access` doc comments call out and fixed by going
  exhaustive; `ForEach` wraps a nested `BatchRequest` exactly like `Batch` does,
  yet `Batch` is admin-classified and `ForEach` is not. (b) *(distinct to this
  lens, verified downstream)* the server's `check_destructive_hmacs`
  (shamir-server `db_handler/admin.rs:637-777`) iterates only `batch.queries` and
  `continue`s on `Batch`/`ForEach`, so a `drop_db`/`grant_role`/`purge_history`
  nested one level inside either wrapper executes with **no** "did-you-mean-it"
  tag at all — this holds even inside `Batch`, whose admin classification is
  correct, so it is not subsumed by 1.1. The superuser coarse gate
  (`handler.rs:512-521`) likewise skips `ForEach`-wrapped admin ops for
  non-superusers (containment verified: each admin handler still runs its own
  `authorize_access` DAC check, so this is gate inconsistency, not privilege
  escalation).
- Failure scenario: A superuser client with a buggy confirm-dialog sends
  `{"batch":{"queries":{"x":{"drop_db":"prod"}}}}` — no `hmac` field anywhere; the
  gate sees only the `Batch` wrapper, `continue`s, and `prod` is dropped without
  the confirmation the protocol promises. Same payload via `for_each` additionally
  skips the coarse-gate error path.
- Suggested fix: In this crate: make `is_admin` an exhaustive `match` (no
  `matches!`) and decide `ForEach`/`Call` explicitly, mirroring `is_write`; provide
  a recursive `for_each_op`/`collect_destructive_ops` helper (the
  `collect_required_access` shape, query_entry.rs:127-155, is the precedent) so
  gates can walk nested bodies. Server side: drive `check_destructive_hmacs`
  through that helper. Add regression tests asserting a nested `DropDb` requires
  its tag.

### 3.5 — medium — Canonical HMAC inputs omit semantically destructive request fields (`cascade`, `dst_path`)
- File:line: `crates/shamir-query-types/src/hmac.rs:101-163`
  (`canonical_drop_db/repo/table`, `canonical_start_migration`), vs
  `src/admin/types/db_ops.rs:35`, `repo_ops.rs:34`, `table_ops.rs:54`
  (`cascade: bool`), `migration_ops.rs:21` (`dst_path: Option<String>`).
- Issue: The tag confirms "drop this repo/table/db" but not "recursively destroy
  everything inside it": `cascade` is absent from all three canonical inputs, and
  `start_migration`'s `dst_path` (a filesystem path for the destination store) is
  absent from its canonical input. A tag legitimately computed for the
  non-cascade / path-less op verifies byte-identically for the maximally
  destructive variant, because the server (verified: `admin.rs:657-680`) recomputes
  from the same under-specified canonical form.
- Failure scenario: A confirmation UI signs `b"drop_repo\0db\0cold"` (plain drop).
  The request is then sent — or mutated by a buggy client/proxy — with
  `cascade: true`; the tag still verifies and every table in the repo is destroyed,
  an action the user never confirmed. Analogously, a migration can be steered to an
  attacker-chosen `dst_path` under a tag computed without it.
- Suggested fix: Include the discriminating fields in the canonical inputs (e.g.
  `b"drop_repo\0db\0repo\0cascade=0|1"`, append `dst_path` or a `"none"` sentinel).
  This is a wire-format change — bump the `hmac key v1` domain-separation string
  (e.g. `v2`) so old tags cannot validate against the new inputs.
- Dedup: **api-wire-protocol 4.5** bundles this defect together with 3.6 (its
  "cascade/if_exists/replace" half is this finding).

### 3.6 — low — Canonical-input encoding ambiguities: interior NULs and empty identifiers
- File:line: `crates/shamir-query-types/src/hmac.rs:89-99` (`join_null`),
  `hmac.rs:184-196` (`canonical_resource_ref`), `hmac.rs:402-408`
  (`canonical_create_scram_user`).
- Issue: `join_null` performs no component validation, so `("a\0b", "c")` and
  `("a", "b\0c")` canonicalize identically — two *different* grant/drop/user tuples
  share one tag. `canonical_resource_ref` has collisions of its own:
  `Function { function: "" }` renders `"fn://"`, identical to the
  `FunctionNamespace` singleton; `FunctionFolder { [] }` renders `"fn:///"`; empty
  db/store/table names also collide across variants (`Database { "" }` →
  `"db://"`). The module itself demonstrates the right rigor for `GroupRef` ("can
  never collide between variants", hmac.rs:70-75) but does not extend it to the
  other encoders, and no test covers NUL-containing or empty components.
- Failure scenario: If names with interior NULs survive anywhere into stored
  principals/resources (only the SCRAM username path is documented as normalised
  via SASLprep — role/db/repo/table/index names are unvalidated `String`s in this
  crate), a tag obtained to confirm op A also confirms the distinct op B, silently
  defeating the intent-confirmation the tag exists to provide.
- Suggested fix: Reject `\0` and empty components in every `canonical_*` helper
  (return `Option`/`Result`), or length-prefix each part instead of NUL-separating.
  Add ambiguity tests to `hmac_tests.rs`.
- Dedup: **api-wire-protocol 4.5** bundles this defect together with 3.5 (its
  "NUL/comma/slash aliasing" half is this finding; it adds that `secret_grants` are
  comma-joined so a grant containing `,` aliases a different grant list).

### 3.7 — low — Destructive-op HMAC coverage has drifted: whole op families are ungated
- File:line: `crates/shamir-query-types/src/admin/types/repo_ops.rs:61-65,87-91`
  (`RenameRepoOp`, `RenameDbOp` — no `hmac`), `table_ops.rs:84-90`
  (`RenameTableOp`), `index_ops.rs:99-126` (`RenameIndexOp`),
  `function_ops.rs:72-90` (`DropFunctionOp`, `RenameFunctionOp`),
  `validator_ops.rs:34-41` (`DropValidatorOp`), `repl_ops.rs:138-161,182-186`
  (`DropReplicationProfileOp`, `DropPublicationOp`, `DropSubscriptionOp` —
  cluster-topology mutations); no `canonical_*` counterparts in `src/hmac.rs`.
- Issue: The confirmation scheme gates `rename_group` and `create_group` but not
  `rename_db`/`rename_repo`/`rename_table`/`rename_index`; it gates `drop_index`
  but not `drop_function` (which destroys code plus its `secret_grants`/
  `net_grants` bindings); it gates nothing in the replication-DDL family or
  validator DDL. There is no single classifier enumerating "ops requiring a tag",
  so each new op family silently ships ungated — the same drift pattern
  `required_access`/`is_write` eliminated by exhaustive matching.
- Failure scenario: An operator fat-fingers `rename_db` on the production console,
  or a scripted client drops the wrong publication — the classes of accident the
  HMAC exists to catch, with no confirmation gate to catch them.
- Suggested fix: Add a `BatchOp::requires_hmac(&self) -> bool` exhaustive match (no
  wildcard) as the single source of truth, generate the `hmac: Option<String>`
  field and canonical helper per gated op, and make the server gate iterate that
  classifier (recursively, per finding 3.4). Related but distinct: api 4.16 (the
  `Option<String>` typing that can't express required-ness).

### 3.8 — low — `BatchLimits` are fully client-supplied; only 3 of 6 fields are server-clamped, and the crate offers no clamping helper
- File:line: `crates/shamir-query-types/src/batch/batch_limits.rs:31-86`, consumed
  via `batch_request.rs:97-99`.
- Issue: The DTO is documented as "Execution limits for security. Prevents DoS
  attacks", but every field arrives from the request. Verified server-side
  (shamir-server `db_handler/handler.rs:489-505`): only `max_result_size`,
  `max_execution_time_secs`, `max_queries` are `min()`-clamped against operator
  caps — `max_nesting_depth`, `max_dependency_depth`, and `max_iterations` are
  taken verbatim from the client. A client can send
  `max_nesting_depth: usize::MAX`, disabling the nesting gate the planner enforces
  (`BatchPlanner::plan`, planner.rs:127-133) and un-binding recursive execution
  depth; `max_iterations: usize::MAX` similarly neutralises the `for_each` runtime
  cap. (The wire-compat half — five of six fields reject partial `limits` maps —
  is api 4.8, a distinct defect.)
- Failure scenario: Authenticated client sends a batch with inflated limits and a
  deeply nested `for_each`/`batch` tree; every depth/iteration backstop that
  assumed a default of 4/1000 is switched off by the attacker themselves — which
  also feeds the recursion depth of finding 3.2.
- Suggested fix: Add a `BatchLimits::clamped_against(server_caps: &BatchLimits) ->
  BatchLimits` helper in this crate (taking per-field `min`) so the server clamps
  all six fields through one auditable call site, and document that these fields
  are *requests*, never authorities.

### 3.9 — low — Derived `Debug` on `DbRequest::ChangePasswordVerify` prints long-term SCRAM credential material
- File:line: `crates/shamir-query-types/src/wire/db_message.rs:29`
  (`#[derive(Debug, ...)] enum DbRequest`), fields at `:159-173`
  (`new_stored_key`, `new_server_key`, `client_proof_old` as plain `Vec<u8>`).
- Issue: `ChangePasswordVerify` carries `new_stored_key`/`new_server_key` — the new
  long-term server-side credential. Possessing the `(stored_key, server_key)` pair
  is sufficient to complete future SCRAM exchanges as that user without the
  password, which is precisely why `User::password_hash` and
  `CreateScramUser::password` are wrapped in `SecretString` (redacted `Debug`,
  zeroize-on-drop) one file over (auth/types.rs:145-170, db_message.rs:57-81). The
  change-password fields get no such treatment: any `tracing::debug!`/log of the
  deserialized `DbRequest` prints the raw key bytes.
- Failure scenario: Verbose request logging on a server (or client SDK debug dump)
  writes `new_stored_key`/`new_server_key` into logs; anyone with log access can
  impersonate the user in subsequent handshakes.
- Suggested fix: Implement a manual `Debug` for `ChangePasswordVerify` that redacts
  the three byte fields (or wrap them in a redacting newtype from
  `shamir_types::secret`), matching the established `SecretString` precedent.

## 4. api-wire-protocol

Lens summary carried forward: well-documented msgpack-first DTO layer with strong
round-trip coverage and genuinely good forward-compat hygiene
(`skip_serializing_if` + `serde(default)` additive fields, the #983
Binary/String untagged fix, base58 RecordId conventions where applied); the
protocol leans on stringly-typed vocabularies, unenforced discriminator-key
uniqueness, and non-injective HMAC canonical forms — patterns that work today only
through review discipline rather than type structure.

### 4.1 — high — `InsertedRecord::get_value_owned("_id")` returns `None` for every deserialized record, contradicting its own doc
- File:line: `crates/shamir-query-types/src/write/inserted_record.rs:81-104`
  (deserialize), `:114-119` (accessor — verified in source).
- Issue: Serialize injects `_id` (base58) into the map from `self.id`; Deserialize
  never extracts it back — it decodes the whole map into `fields` with `id: None`
  (line 98). The deserializer's doc (lines 83-85) explicitly claims "The `_id` key
  is stored in `fields` when present; callers can look it up via
  `get_value_owned(\"_id\")`" — but `get_value_owned` short-circuits on
  `key == "_id"` and returns `self.id.as_ref().map(...)`, which is `None` after any
  deserialization; it never falls through to `self.fields.get("_id")`. Round-trip
  also breaks structural equality: `InsertedRecord { id: Some(r), fields }`
  round-trips to `{ id: None, fields-with-_id-inside }`. The correctness lens
  (1.3, deduped here) adds: the round-trip test acknowledges the asymmetry
  (inserted_record.rs:161-163) and no test asserts `_id` access post-round-trip —
  precisely the path that is broken.
- Failure scenario: A client deserializes a `WriteResult`, then calls
  `record.get_value_owned("_id")` (e.g. to feed `Pagination::After::after_id`,
  whose doc says to echo "the `_id` of the last row") — it silently gets `None` for
  every row even though `_id` is right there in `fields`. The documented access
  path is dead on exactly the side (client) that needs it.
- Suggested fix: In `get_value_owned`, fall through to `self.fields.get("_id")`
  when `self.id` is `None`; better, have the deserializer extract `_id` from the
  map back into `id: Option<RecordId>` so the round-trip is symmetric. Fix the
  stale doc either way. Red test first: round-trip then
  `get_value_owned("_id") == Some(Str(base58))`.
- Dedup: **correctness-tdd 1.3** is the same defect (rated medium there).

### 4.2 — high — `FilterValue` silently coerces msgpack `uint64 > i64::MAX` to lossy `Float` — asymmetric with the crate's own u64 contract
- File:line: `crates/shamir-query-types/src/filter/filter_value.rs:9-81` (untagged,
  `Int(i64)` before `Float(f64)`, no `UInt`/`Big` variant).
- Issue: For an untagged enum, serde tries variants in order: `Int(i64)` rejects a
  uint64 above `i64::MAX`, then `Float(f64)` accepts it via a lossy `as f64`. The
  crate already solved this exact problem on the response side — `QueryRecord`'s
  `visit_u64` (`read/query_record.rs:105-115`, tested up to `u64::MAX` in
  `read/tests/query_record_tests.rs:281-294`) promotes losslessly to
  `QueryValue::Big`, and `Cargo.toml:20-24` documents the "unified u64 contract".
  The request-side value type has no such handling and no test
  (`filter/tests/filter_value_conv_tests.rs` covers only `i64::MAX`).
- Failure scenario: A client (TS `BigInt` via `@msgpack/msgpack`, or any raw
  encoder) sends `{"op":"eq","field":"id","value":18446744073709551615}` as msgpack
  uint64. It decodes to `Float(1.8446744073709552e19)`; the equality filter against
  the stored u64 then never matches (or float-compares to the wrong rows) — silent
  wrong results, no error anywhere.
- Suggested fix: Add a `UInt(u64)` (or `Big(BigInt)`) variant declared before
  `Float`, mirroring `QueryRecord`'s contract — or, if the wire vocabulary must
  stay frozen, give `Float` a strict deserializer that rejects integer payloads
  that don't round-trip exactly (the same technique `de_binary_strict` used for
  `Binary` in #983). Add a `u64::MAX` wire round-trip test. (Same enum as perf
  finding 6.4 — a hand-written key-dispatch deserializer would serve both.)

### 4.3 — medium — `BatchOp` dispatch by key-presence + unknown-field tolerance can silently execute a different op than was sent
- File:line: `crates/shamir-query-types/src/batch/batch_op.rs:286-438` (dispatch),
  `:287-288` (`has("from")` first), `read/read_query.rs:12-46` (every `ReadQuery`
  field but `from` is defaulted).
- Issue: Dispatch selects the first if-chain arm whose key is *present*, then
  decodes with the op struct's serde impl — which ignores unknown fields (no
  `deny_unknown_fields` anywhere in the crate; verifiable by grep). Because
  `ReadQuery` succeeds on any map containing `"from"` and defaults everything else,
  any payload whose key set contains an earlier discriminator (`from`,
  `insert_into`, `update`, `delete_from`, …) is decoded as that op with its
  remaining fields silently dropped. `QueryEntry`'s `#[serde(flatten)]`
  (query_entry.rs:39-40) makes this worse: unknown sibling keys (e.g. a typo'd
  `return_resultt`) are forwarded into the dispatch map and swallowed. Nothing
  enforces discriminator-key uniqueness across the ~70 op structs; the
  `"set"`-last comment (line 433-434) shows the scheme already needed manual
  ordering patches.
- Failure scenario: A future op struct gains a non-discriminator field named
  `from`, `update`, `set`, or `list` (or a third-party client sends
  `{... "from": ...}` intending a new op): the payload decodes as `Read` over table
  `"from"` with all other fields dropped — a different operation runs, silently,
  instead of the client getting "Unknown operation type".
- Suggested fix: Add a compile-time or unit-test invariant: for every op struct,
  its field-name set must intersect the discriminator list exactly at its own
  discriminator (a `static_assert`-style macro or a generated test walking all
  ops). Longer term, wrap ops in an explicit single-key envelope (as `ListOp`/
  `ReplRequest` already do) instead of bare struct merging. At minimum, make
  dispatch verify the payload's key set is *exactly* the chosen op's field set
  modulo known additive fields. (Same function as perf finding 6.1 — the dispatch
  restructure should be designed together with the perf fix.)

### 4.4 — medium — *(dedup: primary write-up at 1.2 — same root-cause defect)* The same `RecordId` identifier rides the wire three different ways (`op_id` bin vs `op_id` string vs `after_id`/`_id` base58)
- File:line: `read/query_result.rs:190` and `read/ddl.rs:14` (`RecordId`, derived
  serde → raw 16-byte msgpack `bin`), `admin/types/index_ops.rs:125,154`
  (`request_id` same), vs `wire/db_message.rs:267-276` (`GetDdlOpStatus { op_id:
  String }`), vs the crate's own stated convention in `read/query_result.rs:74-80`
  ("base58 string … NOT raw msgpack bytes, despite RecordId's own derived
  Serialize").
- Api-lens framing (fold into the 1.2 fix): the base58 bridging modules
  (`id_as_base58_string`, `query_result.rs:98-110`) and `opt_record_id_base58`
  (`read/limit.rs:142-167`) already exist — they're just not applied consistently.
  Serialize every wire-visible `RecordId` field through them; keep `RecordId`'s
  derived serde confined to storage-internal contexts. Crash-recovery polling — the
  flagship use case these fields exist for (#1015) — is the workflow that breaks
  for non-Rust clients.

### 4.5 — medium — HMAC canonical inputs are not injective (NUL/comma/slash aliasing) and don't cover `cascade`/`if_exists`/`replace` modifiers *(one finding spanning two deduped defects — counted under 3.5 (modifiers) and 3.6 (non-injective encoding))*
- File:line: `crates/shamir-query-types/src/hmac.rs:89-99` (`join_null`,
  unescaped), `:184-196` (`canonical_resource_ref`, `/`-joined), `:357-364` (grants
  CSV-joined), `:402-408` (`create_scram_user`: name `"a\0b"` with no roles ≡ name
  `"a"` + role `"b"`); `admin/types/db_ops.rs:35` / `repo_ops.rs:34` /
  `table_ops.rs:54` (`cascade` not in canonical form).
- Issue: The module's stated contract is "Matching tag = confirmation of intent"
  (lines 14-15), but the canonical byte strings are ambiguous: parts are joined
  with `\0` without escaping and no DTO validates names NUL-free, `ResourceRef`
  renders with unescaped `/`, and `secret_grants` are comma-joined (a grant
  containing `,` aliases a different grant list). Separately, the tag for
  `drop_db`/`drop_repo`/`drop_table` hashes only the names — a tag computed for a
  plain drop confirms the `cascade: true` variant (strictly larger blast radius)
  with identical bytes.
- Failure scenario: A tool that signs "drop table X" and shows the user that exact
  intent can have the same bytes replayed against `drop_table X cascade=true`; a
  username containing `\0` makes a `create_scram_user` tag alias two different
  (name, roles) intents. Both are intent-confirmation degradations rather than auth
  breaks (the doc is honest that TLS+SCRAM carry authn), but nothing in the DTO
  layer prevents the ambiguous names.
- Suggested fix: Length-prefix canonical parts (or escape `\0`/`/`/`,`), include
  boolean modifiers in the canonical form, and validate name strings NUL-free at
  the DTO boundary so ambiguity can't be constructed. (Merged fix plan with 3.5 +
  3.6, including the `hmac key v1` → `v2` domain-separation bump.)

### 4.6 — medium — Closed vocabularies modeled as raw `String` where typed enums are the crate's own established pattern
- File:line: instances across `batch/batch_request.rs:64-81` (`isolation`,
  `durability`), `wire/db_message.rs:99-100` (`TxBegin::isolation`),
  `batch/transaction_info.rs:12` (`status`), `filter/filter_enum.rs:163`
  (`Fts.mode`), `filter/filter_enum.rs:190-198` (`Computed.expr_op`/`cmp` — while
  the sibling `ValueCompare` variant uses the typed `ValueCompareOp` at `:205-214`),
  `admin/types/index_ops.rs:39-71` (`index_type`, `fts_tokenizer`, `vector_metric`,
  `vector_quantization`, `functional_op`), `admin/types/function_ops.rs:42-47`
  (`visibility`, `security`), `admin/types/schema_ops.rs:40` (`r#type`), `:211`
  (`CompareDto.op`), `wire/db_message.rs:316-339` / `wire/repl.rs:97-105`
  (`Error.code` vocabulary lives only in doc comments).
- Issue: The crate defines typed, snake_case serde enums for exactly this purpose
  (`ReplDirection`, `ReplMode`, `EventMask`, `AggFunc`, `FkAction`,
  `ValueCompareOp`, `OrderDirection`, `ResultEncoding`) — but a large fraction of
  closed vocabularies are `Option<String>`/`String`. Typos fail only server-side
  (or silently default); clients cannot exhaustively match; renames are invisible
  to the compiler. The `Computed.cmp` case is self-inconsistent within one file.
  Wire error codes (`hmac_required`, `cursor_not_found`, `fk_*`…) are documented in
  prose with no shared constants, so server emitters and client matchers can drift
  undetected.
- Failure scenario: `"serialziable"`, `"cosine"`, `"definer"` typo'd by a
  hand-rolled client: best case an opaque runtime error deep in the engine; worst
  case (fields with `#[serde(default)]` fallback semantics) a silently different
  isolation/metric/security level than intended.
- Suggested fix: Convert closed sets to serde enums (additive-safe: unknown-value
  rejection is the desired behavior for closed sets); for the error-code channel,
  publish `pub const` code strings in this crate so both server and client match
  against one source.

### 4.7 — medium — *(dedup: primary write-up at 3.1 — same root-cause defect; security lens rated it high)* Depth/nesting caps are post-deserialization checks, but serde deserialization itself recurses unbounded
- File:line: `filter/filter_enum.rs:7-9,219-238` (`MAX_FILTER_DEPTH` enforced only
  by opt-in `check_filter_depth` *after* `Deserialize`, whose `Box<Filter>` chain
  recurses per input level); `batch/batch_op.rs:256-277` (`BatchOp::deserialize` →
  `SubBatchOp` → `BatchRequest` → `QueryEntry` → `BatchOp` recursion;
  `max_nesting_depth` is plan-time, `batch/planner.rs:109-117`).
- Api-lens framing (fold into the 3.1 fix): every ingress point must independently
  remember to call `check_filter_depth` / the planner; the DTO itself neither
  bounds nor checks recursion (auth-gated: requires a valid SCRAM session). The
  counting-`Deserializer`-wrapper fix from 3.1 closes this lens's statement of the
  defect as well.

### 4.8 — medium — `BatchLimits` rejects partial `limits` maps — the exact wire-compat failure #662 fixed for one field persists for the other five
- File:line: `crates/shamir-query-types/src/batch/batch_limits.rs:31-69` (only
  `max_iterations` has `#[serde(default = "...")]`), `batch/batch_request.rs:97-99`,
  `batch/planner.rs:109-117`.
- Issue: The `max_iterations` doc (lines 61-66) describes precisely how a mandatory
  field breaks older/partial `limits` payloads ("missing field `max_iterations`")
  — yet `max_queries`, `max_dependency_depth`, `max_execution_time_secs`,
  `max_result_size`, `max_nesting_depth` remain mandatory, so a client wanting to
  lower one knob must send all six. The struct is documented as "security
  limits… prevents DoS" while being a client-authoritative request field (the
  no-clamp half of that is security 3.8, a distinct defect), and the planner's own
  comment documents the budget as per-level (worst case
  `max_queries^max_nesting_depth`).
- Failure scenario: TS client sends `{"limits": {"max_execution_time_secs": 5}}` →
  rejected with `missing field max_queries`; conversely a client can *raise* every
  limit unless the server clamps elsewhere (not visible in this crate).
- Suggested fix: Give every field a `#[serde(default = "...")]` matching `Default`
  (same as #662); rename the doc to "client-requested limits (server clamps)" so
  the wire contract doesn't over-claim security. Land together with 3.8's
  `clamped_against` helper — same struct, same doc rewrite.

### 4.9 — medium — `$query` path syntax silently reserves `.count`/`.length` — record fields with those names are unreachable
- File:line: `crates/shamir-query-types/src/batch/reference.rs:186-191` (field
  named `count`/`length` unconditionally becomes `QueryPath::Count`).
- Issue: `@orders[].length` does not extract each row's `length` field; it returns
  the result count. There is no escape hatch (brackets are index-only; dots are
  field-only), and the substitution is silent — no error, just different data.
  `count` and `length` are common column names.
- Failure scenario: A table with a `count` or `length` column; any `$query`
  reference to it returns the row count instead of the column value — silent wrong
  results in dependent ops.
- Suggested fix: Either reserve the names loudly (plan-time error when the
  referenced alias's projection contains a field with the magic name) or add an
  unambiguous field syntax (e.g. `["length"]` bracketed field segments) and keep
  `.count`/`.length` as sugar.

### 4.10 — low — `InsertedRecord` with a non-map `fields` and no `id` serializes to a shape its own deserializer rejects
- File:line: `crates/shamir-query-types/src/write/inserted_record.rs:63-74`
  (serialize falls through to `fields.serialize`) vs `:102` (`deserialize_map` only).
- Issue: `InsertedRecord { id: None, fields: QueryValue::Str("x") }` serializes as
  a bare msgpack string; deserializing those bytes fails with "expected a map".
  Round-trip is not total; the `{id: Some, non-map}` case works only because the
  `{"_id":…,"_value":…}` envelope happens to be a map (and then decodes with
  `_id`/`_value` stranded inside `fields` — see 4.1).
- Failure scenario: a row with a scalar payload and no id round-trips into a
  deserialization error in the same process that serialized it.
- Suggested fix: Always emit the two-key envelope when `fields` is not a map, or
  make the visitor accept scalars (`deserialize_any` + non-map arm wrapping into
  `fields`).

### 4.11 — low — `QueryRecord` wire shape aliases `Direct(QueryValue::Bin)` and `IdBytes`; `as_value()` silently substitutes `Null` for opaque rows
- File:line: `crates/shamir-query-types/src/read/query_record.rs:43,58-67,87-92,189-196`.
- Issue: Deserialization routes every msgpack `bin` payload to `IdBytes`, so
  `Direct(Bin(x))` round-trips as `IdBytes(x)` — variant identity is not preserved
  (top-level bin rows are indistinguishable from id-keyed pass-through bytes). And
  `as_value()` on `IdBytes` returns `QueryValue::Null` as a "safe sentinel": a
  caller that forwards `as_value()` output (serialize, map into another op)
  silently turns a real row into `Null` rather than failing. (Distinct from
  error-handling 5.3's `From<QueryValue> for FilterValue` Null substitution — a
  different type, same silent-substitution pattern.)
- Failure scenario: an intermediary forwards `as_value()` output into another op;
  opaque rows become `Null` values downstream with no error.
- Suggested fix: Document (or forbid via the builder) top-level binary rows on the
  Name path, and make `as_value()` on `IdBytes` return `Option`/`Result` (or
  panic-free explicit `de_intern` API) instead of inventing a `Null` row.

### 4.12 — low — Wire tag conventions are inconsistent across the protocol
- File:line: `read/limit.rs:14-15` (`tag = "mode"`, no `rename_all` → PascalCase
  `LimitOffset`/`Page`/`After`/`None`, self-acknowledged at `:41-43`) vs snake_case
  tags in `filter_enum.rs:13` (`op`), `wire/db_message.rs:30,281` (`op`/`kind`),
  `select.rs:50` (`type`), `wire/repl.rs:25,70` (`repl_op`/`repl_kind`),
  `auth/types.rs:15` (`scope`); `At` externally tagged (`temporal.rs:12-18`);
  `DeliverMode` externally tagged mixing bare strings and single-key maps
  (`subscribe/deliver_mode.rs`).
- Issue: Four different tag key names and two casings for variant tags in one
  protocol. Not a correctness bug, but a permanent tax on hand-writing clients,
  protocol docs, and any codegen — and it makes "add an enum" decisions ad hoc each
  time.
- Failure scenario: every new hand-rolled client re-implements per-enum tag
  handling; every protocol doc re-explains the casing exceptions.
- Suggested fix: Standardize on one tag key (e.g. `"op"`) + `snake_case` for new
  enums; schedule `Pagination`'s PascalCase tags for deprecation-by-alias (accept
  both, emit canonical).

### 4.13 — low — `FieldPath` accepts a bare string in filters but requires arrays in SELECT/ORDER BY/GROUP BY/aggregate field
- File:line: `filter/filter_enum.rs:17` + `:251-265` (`de_field_path`:
  string-or-array) vs `read/select.rs:57` (`SelectItem::Field.path`),
  `read/order_by.rs:38`, `read/group_by.rs:11`, `read/agg.rs:23` (plain `FieldPath`
  = `Vec<String>`, array-only).
- Issue: The same conceptual field reference has two wire grammars:
  `{"op":"eq","field":"id"}` works, but `{"type":"field","path":"id"}` fails — it
  must be `{"path":["id"]}`. Deserialize-from-string is only wired into `Filter`.
- Failure scenario: Client authors (or the TS SDK) naturally reuse the string
  shorthand from WHERE in projections and get opaque "invalid type: string,
  expected a sequence" errors.
- Suggested fix: Reuse `de_field_path` on every `FieldPath` wire field
  (serialization already always emits the canonical array).

### 4.14 — low — `InsertOp` carries two parallel record channels with unspecified result ordering
- File:line: `crates/shamir-query-types/src/write/types.rs:81-93` (`values` +
  `records_idmsgpack`, "both may be present in one op (different records)").
- Issue: The DTO does not define the interleaving/order of returned rows when both
  channels are non-empty (which channel's rows come first in `WriteResult.records`
  / how `QueryResult::versions` aligns), leaving the pass-through v2 path's
  client-visible contract implicit.
- Failure scenario: a client using both channels cannot correlate `WriteResult`
  rows back to inputs deterministically across server versions.
- Suggested fix: Document the canonical order (e.g. `values` rows then
  `records_idmsgpack` rows) in the field docs, or make the channels mutually
  exclusive at the DTO level once v2 clients settle.

### 4.15 — low — `query_version` negotiation coverage is inconsistent within `DbRequest`
- File:line: `wire/db_message.rs:39-47,89-116,222-234`
  (Execute/TxBegin/TxExecute/CreateCursor carry it) vs `:119-132,57-81,188-211`
  (TxCommit/TxRollback/CreateScramUser/SetSuperuser/SetReplicator don't).
- Issue: A future v3 client talking to a v2 server is rejected on `Execute` but its
  `TxCommit`/admin ops are accepted unversioned — the gate covers only part of the
  surface that shares `BatchRequest`/behavior semantics.
- Failure scenario: protocol negotiation silently passes for half the request
  surface, so a v3 semantic change to admin ops ships unguarded by version checks.
- Suggested fix: Either add `query_version` (with the same `default`) to the
  remaining stateful ops, or document why only batch-carrying ops need it.

### 4.16 — low — "Always required" HMAC fields are `Option<String>` — required-ness exists only in prose and runtime gates
- File:line: `wire/db_message.rs:77-80,193-196,208-210` ("always required
  (unconditional)" over `hmac: Option<String>`); same shape throughout
  `auth/types.rs`, `admin/types/*`.
- Issue: The documented-unconditional gates (`CreateScramUser`, `SetSuperuser`,
  `SetReplicator`) are unenforceable at the type level; every consumer must
  re-implement the "None ⇒ `hmac_required`" check, and the wire can never express
  "this field must be present". (Accepted-verbatim rationale exists for
  round-tripping — `DropUserOp`'s "Option purely to allow types to roundtrip
  uncheckedly" — but it's applied uniformly, including where no round-trip need
  exists.) Related but distinct from 3.7 (which ops are gated at all).
- Failure scenario: a new consumer forgets the None check; an unconditional gate
  silently becomes optional for that path.
- Suggested fix: Keep `Option` for backwards wire compat, but add
  `#[must_use]`-style constructor helpers or a `validate_hmacs()` method on the ops
  that centralizes the required-ness the docs describe.

### 4.17 — nit — Doc/wire mismatches and stale references *(composite; subsumes style 7.7 and 7.8)*
- File:line: `wire/db_message.rs:330-332` (error-code list names `fk_restrict`
  twice — the duplicate entry is a copy-paste drift in the authoritative error-code
  enumeration); `hmac.rs:55-68` (canonical-inputs markdown table split in two by
  the explanatory paragraphs — the second half renders header-less, so the
  documented canonical inputs for all group/function/superuser/SCRAM ops are
  effectively unformatted in rustdoc); `admin/types/validator_ops.rs:57-62,78`
  (examples show nested `"table": {"db":…,"repo":…,"table":…}` objects; the structs
  take flat `db`/`repo`/`table: String`); `batch/batch_limits.rs:9-18` (doc table
  lists 5 defaults, struct has 6 — `max_nesting_depth` missing); `read/limit.rs:320`
  (comment references `has_next_hint`; the method is `with_has_next`);
  `wire/mod.rs:19-20` (`CURRENT_QUERY_LANG_VERSION` re-exported at the `wire` root,
  `CURRENT_REPL_PROTO_VER` only reachable via `wire::repl::`).
- Issue: Protocol-reference documentation that drifts from the structs (wrong
  examples, split tables, duplicate vocabulary entries) is what hand-rolled clients
  are built from.
- Failure scenario: a client author implements against the nested-table example or
  the duplicated `fk_restrict` list and ships a client that cannot talk to the
  server.
- Suggested fix: Fix each in place; re-export `CURRENT_REPL_PROTO_VER` beside
  `CURRENT_QUERY_LANG_VERSION`.
- Dedup: **style-claude-md 7.7** (`fk_restrict` duplication) and **7.8** (the
  orphaned hmac.rs table) are items within this composite finding — counted once,
  here.

### 4.18 — low — *(dedup: primary write-up at 7.2)* Inline `#[cfg(test)] mod tests` in implementation files, despite the documented `tests/` layout and existing sibling test files
- File:line: `crates/shamir-query-types/src/read/query_record.rs:302-434`;
  `src/write/inserted_record.rs:134-214`.
- Api-lens framing (folded into 7.2): these particular tests are wire round-trip
  tests, i.e. squarely this crate's protocol contract — the duplication risk is to
  the wire contract, not just style.

## 5. error-handling-lifecycle

Lens summary carried forward: pure-DTO crate, no I/O resource lifecycle to manage;
error discipline generally strong — no `anyhow`/`Box<dyn Error>` leakage, serde
decode errors flow through `de::Error::custom`, and the only three non-test
`unwrap`/`expect` sites sit on documented invariants. The real exposure is
DoS-surface (deduped into §3) plus two conversion/arithmetic paths that silently
degrade and test/convention gaps.

### 5.1 — medium — *(dedup: primary write-up at 3.2 — same root-cause defect)* Unbounded recursion in `detect_cycle` / `calculate_max_depth` — stack overflow aborts the server
- File:line: `crates/shamir-query-types/src/batch/planner.rs:671-703` (dfs),
  `planner.rs:721-747` (`depth`), contrast `planner.rs:69` + `planner.rs:770-773`
  (`NESTING_WALK_LIMIT` iterative walk).
- Error-lens framing (folded into 3.2): `BatchPlanner::plan` deliberately made the
  sub-batch nesting walk iterative with a hard 64-deep cap, commented "so a
  malicious deeply-nested payload cannot blow the call stack" — but `detect_cycle`'s
  DFS (line 692) and `calculate_max_depth`'s `depth()` (lines 735-737) still
  recurse once per node; depth is bounded only by `limits.max_queries`, which the
  crate never caps. The `max_queries_per_batch = 500_000` linear-chain scenario and
  the abort-not-panic consequence are carried in 3.2's failure scenario; the fix
  (worklist conversion or chain-length cap mirroring `NESTING_WALK_LIMIT`,
  `topological_sort` as the in-file iterative pattern) is 3.2's.

### 5.2 — low — *(dedup: primary write-up at 3.3 — same root-cause defect)* `check_filter_depth` does not descend into `FilterValue` operands — doc claims `$cond` coverage it doesn't have
- File:line: `crates/shamir-query-types/src/filter/filter_enum.rs:216-238`.
- Error-lens framing (folded into 3.3): today the only backstop is rmp-serde's own
  ~1024-container decode depth limit — "an accident of the codec, not this crate's
  64-deep contract, and one that silently changes if the wire codec ever does";
  downstream, every recursive walk (planner
  `extract_deps_from_filter`/`filter_value_contains_field_based_comparison`,
  engine compile/eval) runs the hidden depth. Fix and test per 3.3.

### 5.3 — low — `From<QueryValue> for FilterValue` silently substitutes `Null` in release builds
- File:line: `crates/shamir-query-types/src/filter/filter_value.rs:257-279`
  (tier 3, lines 270-278).
- Issue: When both the direct conversion and the msgpack round-trip fail, the impl
  returns `FilterValue::Null` and guards the case with only a
  `debug_assert!(false, …)`. CLAUDE.md's rule is "Return `Result<T, E>` … avoid
  silent failure"; a `From` impl that can fail must not exist as infallible —
  production builds get a wrong value (`Null`) with no error, no log, and no trace.
  Current callers are mostly literal conversions (builders/tests), and the live
  DDL-default path deserializes `FilterValue` directly, so this is latent rather
  than active — but `QueryValue::Map` is exactly the tier-2 input a future caller
  can hand it, and a malformed expression default would silently become a NULL
  default stamped on writes.
- Failure scenario: Any future call site doing `let fv: FilterValue =
  client_map.into();` with a map that doesn't decode as a `FilterValue` gets `Null`
  in production — a silent data substitution — while debug builds panic at the same
  site, i.e. the failure mode differs by build profile.
- Suggested fix: Remove the tier-3 fallback: make the fallible conversion an
  explicit `try_from`-style `Result`/`Option` API (the crate already has the right
  shape in `query_value_to_filter_value -> Option`), and keep `From` only for the
  provably-infallible literal conversions.

### 5.4 — low — *(dedup: primary write-up at 1.6 — same root-cause defect)* Non-saturating arithmetic on client-controlled `u64` pagination fields
- File:line: `crates/shamir-query-types/src/read/limit.rs:180`
  (`page.saturating_sub(1) * page_size`), `limit.rs:294` (`skip + page_size <
  total`).
- Error-lens framing (folded into 1.6): everything around these sites is saturating
  (`saturating_sub` on the same line; the planner's ForEach gate uses
  `saturating_mul`), so these two sites are inconsistent with the crate's own
  arithmetic discipline. Fix per 1.6.

### 5.5 — low — *(dedup: primary write-up at 1.4 — same root-cause defect)* Missing in-crate error-path tests: headline planner errors, `QueryReference::parse`, non-finite float rejection
- File:line: `crates/shamir-query-types/src/batch/tests/planner_tests.rs` (error
  variants covered), `src/batch/tests/mod.rs:1-8` (no `reference_tests`),
  `src/read/query_record.rs:117-122`.
- Error-lens framing (folded into 1.4): three concrete gaps where error paths have
  no coverage under `./scripts/test.sh -p shamir-query-types` — `BatchPlanner::plan`'s
  own `# Errors` doc lists `TooManyQueries`, `UnknownAlias`, `CircularDependency`,
  `TooDeep`, but this crate's planner tests exercise only `NestingTooDeep`,
  `AfterPathIgnored`, `InvalidWhenFilter`, `InvalidCondCondition` (and ForEach's
  `TooManyQueries` gate); `QueryReference::parse` and all seven
  `ReferenceParseError` variants have zero tests in this crate;
  `QueryRecordVisitor::visit_f64`'s explicit rejection of non-finite floats
  (`de::Error::custom("non-finite float in QueryRecord")`) is untested anywhere in
  the crate. Port plan per 1.4.

### 5.6 — low — thiserror convention deviation: hand-rolled Display/Error impls and `Result<(), String>` public APIs
- File:line: `crates/shamir-query-types/src/batch/batch_error.rs:245-368`,
  `src/batch/reference.rs:243-275`, `src/filter/filter_enum.rs:219`
  (`check_filter_depth -> Result<(), String>`), `src/admin/types/retention.rs:40-47`
  (`Retention::validate -> Result<(), String>`).
- Issue: CLAUDE.md says "`thiserror` for library error enums"; this crate
  hand-writes `Display` + empty `std::error::Error` impls for both of its error
  enums (`BatchError` ~120 lines, `ReferenceParseError`), and thiserror is not even
  a dependency. Two public validators return untyped `Result<(), String>`, which
  callers cannot match on programmatically (the server has to stringly
  re-classify them). The crate's minimal-dependency stance may justify this, but
  nothing in CLAUDE.md carves out that exception and no comment documents the
  deviation (the workspace sweep flags the same pattern repo-wide).
- Failure scenario: a server retry/classification policy must string-match English
  messages ("filter nesting depth exceeds 64") to distinguish error classes; a
  wording change becomes a behavioral break.
- Suggested fix: Either adopt `thiserror` for the two enums (it's a proc-macro with
  no runtime footprint) or add a short doc comment on each enum stating the
  deliberate no-macro rationale; give `check_filter_depth`/`Retention::validate`
  typed error enums (or at minimum a `#[non_exhaustive]` error type) so callers can
  match rather than parse strings.

### 5.7 — nit — `Pagination`'s `PartialEq` can panic via `key_bytes`' `expect`
- File:line: `crates/shamir-query-types/src/read/limit.rs:130-133`.
- Issue: `key_bytes` asserts "serializing Vec<QueryValue> is infallible", but
  rmp-serde's encoder has a `DepthLimitExceeded` failure mode (~1024 nested
  containers). Wire-decoded seek tuples can't exceed the decoder's symmetric limit,
  so this is practically unreachable — but any in-process construction of a
  >1024-deep `QueryValue` turns a comparison (`pagination == other`) into a panic
  in a trait impl.
- Failure scenario: an in-process >1024-deep seek tuple makes a plain `==` abort a
  request via panic in `PartialEq`.
- Suggested fix: Compare with a fallback (`unwrap_or_default()` on the encode
  result) or document the invariant next to the `expect` the way `hmac.rs` does.
  (Same function as perf 6.8 — one comparator fix can close both.)

### 5.8 — nit — `expect` in HMAC tag compute/verify (acceptable, but undocumented-as-invariant)
- File:line: `crates/shamir-query-types/src/hmac.rs:414-415`, `hmac.rs:428-429`.
- Issue: `Mac::new_from_slice(key).expect("HMAC-SHA256 accepts any key length")` —
  genuinely infallible for the fixed `&[u8; 32]` key (HMAC only rejects keys larger
  than the block size), and thus within CLAUDE.md's "invariant" allowance, but
  these are the crate's only reachable-by-panic `pub fn` bodies on both client and
  server paths and carry no inline comment naming them as invariants.
- Failure scenario: none at runtime; a future audit or refactor re-litigates the
  `expect`s (or generalizes the key type and makes it real).
- Suggested fix: Add the one-line "32 < SHA-256 block size (64) — infallible by
  construction" comment, or hoist a `Hmac<Sha256>`-per-key construction to make the
  invariant structural.

### 5.9 — nit — *(dedup: primary write-up at 1.10 — same root-cause defect)* `TableRef` deserialization silently ignores trailing seq elements
- File:line: `crates/shamir-query-types/src/table_ref.rs:71-79`.
- Error-lens framing (folded into 1.10): a lenient-accept of malformed input in a
  crate that is otherwise strict about rejecting ambiguous wire shapes (cf.
  `de_binary_strict`, `AfterPathIgnored`); fix per 1.10.

## 6. performance-hotpath

Lens summary carried forward: hot paths are wire (de)serialization, the batch
planner, and result-row serialization. Bench coverage exists only for
`BatchPlanner::plan` (`benches/batch_planner.rs`) — none of the paths below are
benched, and findings 6.1/6.2 should land with a `bench_scale_tool::Harness` bench
first (baseline) per the repo's /opti workflow. The nested-batch exponential budget
(`max_queries^max_nesting_depth`) is a known, documented trade-off (#666) and is
not re-litigated here. (~350 functional tests exist; the gaps that matter are
benches, not correctness round-trips.)

### 6.1 — high — `BatchOp::deserialize` — triple codec round-trip + key clones + linear dispatch chain per op
- File:line: `src/batch/batch_op.rs:256-284` (round-trip at 262–277; `keys` clone
  266–269; `has()` 270; dispatch chain 287–438).
- Issue: Every batch op deserializes as: (1) buffer the whole op into a
  `QueryValue`, (2) `rmp_serde::to_vec_named(&qv)` — a full re-encode of the entire
  op payload into a fresh `Vec<u8>`, (3) `rmp_serde::from_slice` — a full re-decode
  into the typed op struct. That is 3 codec passes and ≥2 full-payload allocations
  per op, paid on every `Execute`/`TxExecute` request, and multiplied by nested
  `Batch`/`ForEach` bodies (each nested level's entries each pay it again). On top:
  `keys: Vec<String> = m.keys().cloned().collect()` clones every top-level key
  string per op, and `has = |k| keys.iter().any(...)` is a linear scan — with ~75
  sequential probes ("set" is deliberately probed last, after every other
  discriminator), a late-chain op pays ~75×K string comparisons. The enclosing
  `DbRequest` is also internally-tagged (`#[serde(tag = "op")]`,
  `src/wire/db_message.rs:30`), which adds its own full-content buffering pass
  around all of this.
- Failure scenario: A 5 MB INSERT-heavy batch pays ~10 MB of avoidable encode/decode
  work plus thousands of extra allocations on the server's decode path per request;
  deep ForEach nesting multiplies it per level.
- Suggested fix: Borrow the keys (`m.keys().any(|s| s == k)` — no `Vec<String>`
  clone). Replace the 75-branch `has()` chain with a single pass over the map's
  keys matched against a static discriminator table (lazy-init `FxMap<&str,
  OpKind>` or a `match`). Longer term, feed each op struct's `Deserialize` directly
  from the already-decoded `QueryValue` via a Content-style bridge (or an
  externally-tagged op tag in a future query-lang version) to eliminate the msgpack
  re-encode. Add a bench for this path — `benches/` currently covers only the
  planner. (Same function as api 4.3 — the discriminator-table restructure can
  close the dispatch-correctness invariant too.)

### 6.2 — medium — `InsertedRecord::serialize` — per-record `Vec` collect + sort + base58, contradicting the "allocation-free" module claim
- File:line: `src/write/inserted_record.rs:29-61` (pairs collect/sort at 39–40;
  `id.to_string()` at 32).
- Issue: For every returned row, serialization collects `Vec<(&String, &Value)>` of
  all fields and `sort_unstable_by_key`s them — O(F log F) comparisons plus one
  `Vec` allocation per record per serialization, plus a base58 `RecordId::to_string()`
  per record. A write returning N rows × F fields pays O(N·F log F) + 2N
  allocations per wire encode, and every re-serialization (replication fan-out to S
  subscribers re-encodes the same rows) pays it again. The module doc
  (inserted_record.rs:1-12) claims "Allocation-free write-result record for
  INSERT/UPSERT hot paths" — true for construction, false for serialization.
- Failure scenario: `INSERT … returning` 10k rows × 20 fields → 10k sorts + 20k
  allocations per response, ×S subscribers under replication.
- Suggested fix: Establish the sorted-key invariant once at construction (the
  engine builds these rows — sort the key order when the `WriteResult` is
  assembled), or cache a sorted key permutation alongside `fields`; keep
  per-serialize work a linear emit. Bench first (see section preamble).

### 6.3 — medium — Filter depth guard does not cover `FilterValue::Cond` nesting — unbounded deserialize-time recursion *(both halves deduped: decode-recursion half → 3.1; guard-coverage half → 3.3)*
- File:line: `src/filter/filter_enum.rs:216-238` (`check_filter_depth` walks only
  `And`/`Or`/`Not`); `src/filter/filter_value.rs:71-74` + `src/filter/cond.rs:40-50`
  (mutual recursion `FilterValue::Cond → Cond.condition: Box<Filter> → Filter`).
- Perf-lens framing (folded into 3.1/3.3): a `$cond` chain threaded through values
  reports depth 1 regardless of true depth; the guard can only run *after*
  deserialization, but `Filter`/`FilterValue` deserialization itself recurses
  Cond↔Filter↔FilterValue with no depth bound — each wire level costs ~40 bytes
  (`{"$cond":{"if":…`), so a modest payload builds tens of thousands of stack
  frames (untagged `FilterValue` additionally buffers a serde `Content` per level)
  and can overflow the decode thread's stack before `MAX_FILTER_DEPTH` is ever
  consulted. The doc at `filter_enum.rs:7-9` claims the cap prevents "stack
  overflow post-handshake", which it cannot for value-tree nesting. One fix (decode-
  time depth counter + extended iterative guard) closes both halves; the perf
  concern (serde `Content` buffering per level) is additionally addressed by 6.4's
  hand-written dispatch.

### 6.4 — medium — `FilterValue` — 13-variant `#[serde(untagged)]` enum: content buffering + ~6 failed map-shaped trials per marker value
- File:line: `src/filter/filter_value.rs:9-81` (same pattern repeated for `FnCall`
  `src/filter/fn_call.rs:22-33`, plus `GroupRef`/`ResourceRef`/`NumDto`/
  `SelectExprValue`/`AggregateField`).
- Issue: serde's untagged machinery buffers the whole value into `Content` and
  tries variants in declaration order. The marker variants (`FieldRef`, `QueryRef`,
  `FnCall`, `Expr`, `Cond`, `Param`) are declared last, so every `$query`/`$param`/
  `$fn` reference inside every WHERE / `when` / `set` / `bind` value pays full
  buffering plus ~6 failed struct-variant decode attempts over the buffered content.
  This is per-filter-value, per-request wire cost — a linear constant the O(x→0)
  pillar would rather not pay.
- Failure scenario: a filter-heavy batch with many `$query`/`$cond` markers pays
  ~7× the necessary decode work per marker value; multiplied by ForEach re-planning
  (per iteration, up to 1000).
- Suggested fix: Replace untagged with a hand-written `Deserialize` that dispatches
  on the map's reserved key (`$query`/`$fn`/`$cond`/`$expr`/`$param`/`$ref`) the way
  `de_binary_strict` already hand-routes `Binary` — wire shape unchanged,
  single-pass decode. Literal variants already fail fast; the win is for marker
  values. (Same enum as api 4.2 — one hand-written deserializer can fix the u64
  coercion and the cost together.)

### 6.5 — medium — `QueryRecord::get_value_{i64,u64,bool}` — deep-clones the whole `Inserted` record per scalar lookup
- File:line: `src/read/query_record.rs:218-227` (`get_value_owned` → `as_value()` =
  `rec.fields.clone()`), `246-284`.
- Issue: For `QueryRecord::Inserted`, each i64/u64/bool lookup routes through
  `get_value_owned` → `as_value()`, which makes a full deep clone of the record's
  `fields` `QueryValue`, then clones the one found value — work proportional to the
  *whole record* per scalar read. A caller reading k fields of n returned rows pays
  O(n·k·record_size) — a hidden near-quadratic in helpers. Inconsistent with
  `get_value_str` (lines 235–241), which borrows from `rec.fields` at zero cost;
  the cheap path exists one match-arm away.
- Failure scenario: a result-processing loop reading 5 scalar fields of 10k rows
  deep-clones 50k full records.
- Suggested fix: Mirror `get_value_str`: `QueryRecord::Inserted(rec) =>
  rec.fields.get(key).and_then(QueryValue::as_i64)` (likewise `as_u64`/`as_bool`).

### 6.6 — low — Batch planner — redundant alias-set clone and repeated String re-cloning through the plan
- File:line: `src/batch/planner.rs:163-164` (`aliases` TSet + `alias_order` both
  `keys().cloned()`), `200-203` & `226` (deps inserted into `provenance`, then
  re-cloned into `deps`), `238-239` (`alias.clone()` per insert), `816-817`
  (`deps[k].len()` — second hash lookup per key), `857` (stages re-clone every
  alias).
- Issue: `aliases` duplicates information `queries` already has —
  `queries.contains_key(dep)` answers the same validation with zero allocation.
  Each alias string ends up cloned ~4× per plan (aliases, alias_order,
  dependencies/edge_provenance keys, stages). Absolute cost is bounded by
  `max_queries` (50/level), but the planner re-runs per nested batch and per
  ForEach iteration (engine re-plans the body up to `max_iterations` = 1000 times),
  so the churn multiplies.
- Failure scenario: bulk-load workloads with max-iterations ForEach bodies pay
  4×-alias-clone churn per re-plan, up to 1000× per top-level op.
- Suggested fix: Drop the `aliases` set and use `queries.contains_key`; drain
  `provenance` keys into `deps` instead of re-cloning; iterate `deps.iter()` once
  when seeding `in_degree`; consider `Rc<str>`/`Box<str>` keys in `BatchPlan` if
  clones remain.

### 6.7 — low — Three separate full-tree recursive walks per request: `is_write`, `distinct_repos`, `collect_required_access`
- File:line: `src/batch/batch_op.rs:764,771` (`is_write` recursion over
  `Batch`/`ForEach` bodies); `src/batch/query_entry.rs:93-155`
  (`repos.insert(tr.repo.clone())` at 105; un-deduped access `Vec` at 127-134).
- Issue: Each helper independently re-walks the entire op tree; `is_write` is
  invoked per-op by classification paths, so in the worst case (all-read nested
  batches at max fanout/depth, within the documented 50^4 budget) total visits
  approach the square of tree nodes. Also `collect_repos` clones the repo `String`
  per entry even when already present, and `collect_required_access` returns
  duplicates, so the engine's auth pre-check re-validates the same `(Action,
  ResourcePath)` repeatedly.
- Failure scenario: deeply nested all-read batches pay superlinear classification
  work per request; auth pre-checks re-validate duplicate grants.
- Suggested fix: One fused classification walk computing (repos, required_access,
  has_write) in a single pass; `contains` check before insert for repos; dedup the
  access list. **Coordinate with security 3.2** — these are the same walkers that
  need depth-bounding; land the fused, bounded walk once.

### 6.8 — low — `Pagination::eq` (`After`) — two msgpack encodes per equality comparison
- File:line: `src/read/limit.rs:123`, `131-133`.
- Issue: `key_bytes(k1) == key_bytes(k2)` allocates and fully serializes both seek
  tuples on every `==`. Harmless in tests; costly if `After` pagination ever lands
  in a cache key / request-dedup hot path.
- Failure scenario: `After` values in a dedup/cache-key path pay 2 encodes per
  comparison, alloc-heavy.
- Suggested fix: Compare element-wise (equal-length short-circuit then a canonical
  `QueryValue` comparator), or compute the encoded form once at construction and
  store it. (Same function as error-handling 5.7 — the comparator rewrite can also
  remove the `expect`.)

### 6.9 — low — Plan-time marker decode pays a msgpack round-trip per `$query`/`$fn`/`$cond`/`$expr` marker
- File:line: `src/batch/planner.rs:392-419` (`rmp_serde::to_vec_named(value)` +
  `from_slice::<FilterValue>` per marker map).
- Issue: Each marker map found while walking write values is re-encoded to msgpack
  and re-decoded as a `FilterValue` (2 allocations + 2 codec passes) just to reuse
  `extract_deps_from_filter_value`. Multiplied by engine-side ForEach re-planning
  (per iteration, up to 1000).
- Failure scenario: insert-heavy batches with many `$query` markers pay 2 codec
  passes per marker per re-plan.
- Suggested fix: Decode the marker directly from the `QueryValue` map (match on the
  reserved key, read `alias`/`path`/`args` fields) — O(marker size), no codec — or
  cache decoded markers within one plan pass. (Pairs naturally with 6.4's
  single-pass `FilterValue` deserializer.)

### 6.10 — nit — Per-construction `"main"` String allocations
- File:line: `src/table_ref.rs:21` (`DEFAULT_REPO.to_string()`); `default_repo()`
  in `src/call/mod.rs:13`, `src/admin/types/table_ops.rs:9`, `index_ops.rs:9`, and
  siblings.
- Issue: Every `TableRef::new` and every defaulted `repo` field allocates a fresh
  `"main"` `String` — one avoidable allocation per op on the request construction
  path.
- Failure scenario: cosmetic unless op-construction throughput matters.
- Suggested fix: `Cow<'static, str>` for the repo field, or a shared interned
  default.

## 7. style-claude-md

Lens summary carried forward: the module/test skeleton is largely exemplary — every
module has a `tests/` directory whose `mod.rs` is a re-export-only manifest, tests
are split by topic, and the bench uses the mandated `bench_scale_tool::Harness`.
Two hard CLAUDE.md rules are breached (types in `mod.rs`; inline test blocks), and
the imports-at-top rule is violated 22 times (ten mid-function `use` statements
across four implementation files, plus twelve in six standalone test files), none
with a documented exception justification. (Workspace note: the sweep itself
observed these same violation classes were rated "high" for this crate but
"medium"/"low" elsewhere — the ratings below are as filed by this crate's style
reviewer.)

### 7.1 — high — Types defined inside mod.rs (re-export-only rule breach)
- File:line: `crates/shamir-query-types/src/validator/mod.rs:5-30` (verified in
  source: `WriteOp` lines 9-16, `ValidationError` lines 23-30, plus their `use`
  imports at 5-6), `crates/shamir-query-types/src/call/mod.rs:13-43` (verified:
  the entire module in one `mod.rs` — `CallOp` at 31-43 and its `default_repo`
  helper at 13-15 instead of a `call/call_op.rs` sibling).
- Issue: CLAUDE.md (Discipline rules): "`mod.rs` files contain re-exports only.
  Types and logic live in sibling files." This also breaches the companion "one
  file = one primary export" rule: two distinct public types (a validator-trigger
  enum and an error DTO) in one file. Every other module in this crate (read/,
  batch/, wire/, write/, filter/, subscribe/, admin/) follows the sibling-file
  convention, so these two are outliers.
- Failure scenario: contributors copying `validator/mod.rs` as a template propagate
  the mod.rs-with-types pattern; `git blame` on `WriteOp`/`ValidationError`
  conflates them with module-wiring churn, defeating the rule's stated goal of
  atomic diffs and meaningful blame.
- Suggested fix: Move `WriteOp` to `validator/write_op.rs` and `ValidationError` to
  `validator/validation_error.rs` (their tests in `validator/tests/` already split
  exactly along these lines: `write_op_tests.rs`, `validation_error_tests.rs`);
  move `CallOp` to `call/call_op.rs`. Both `mod.rs` files become re-export-only
  (`pub use write_op::WriteOp; pub use validation_error::ValidationError;`). All
  external `use crate::validator::{...}` / `crate::call::CallOp` paths stay valid.

### 7.2 — high — Inline `#[cfg(test)] mod tests { ... }` embedded in implementation files *(primary; subsumes correctness 1.7 and api 4.18)*
- File:line: `crates/shamir-query-types/src/read/query_record.rs:302-434`,
  `crates/shamir-query-types/src/write/inserted_record.rs:134-214`.
- Issue: Test-organisation rule 5: "Never embed `#[cfg(test)] mod tests { ... }`
  inline inside implementation files. Move them to the `tests/` directory." Both
  modules already have compliant `tests/` directories wired via the parent
  `mod.rs` — and the coverage has drifted into overlap: `query_record.rs`'s inline
  block holds the msgpack round-trip tests while
  `read/tests/query_record_tests.rs` holds the accessor tests for the *same type*;
  `inserted_record.rs`'s inline `inserted_record_sorted_key_order` /
  `inserted_record_no_id_serialization` duplicate the sorted-key and no-id cases
  already pinned by `write/tests/inserted_record_tests.rs`
  (`set_insert_map_with_id_and_created`, `update_returning_base_only`,
  `no_id_non_map_value_direct_serialization`); the correctness lens adds
  `partial_eq_direct_vs_inserted` vs the tests-dir accessor suite. These are the
  only two inline test modules in the crate (verified by grep). The api lens adds:
  these particular tests are wire round-trip tests, i.e. squarely the crate's
  protocol contract.
- Failure scenario: A wire-contract change (e.g. `_id` injection order) must be
  applied in two places for one type; updating only the external file leaves the
  stale inline assertion or vice-versa, and a future dev looking for "the tests for
  InsertedRecord" finds only one of the two halves.
- Suggested fix: Move the inline blocks into the existing `tests/` directories as
  new topic files (e.g. `read/tests/query_record_serde_tests.rs`,
  `write/tests/inserted_record_roundtrip_tests.rs`), deduplicate the overlapping
  assertions against the existing files, and register them in the respective
  `tests/mod.rs` manifests.

### 7.3 — medium — Mid-function `use` statements in implementation files (imports-at-top breach)
- File:line: `crates/shamir-query-types/src/hmac.rs:79,185,271,303,412-413,426-427`;
  `crates/shamir-query-types/src/batch/planner.rs:372,585,619`;
  `crates/shamir-query-types/src/batch/batch_op.rs:260`;
  `crates/shamir-query-types/src/table_ref.rs:52`.
- Issue: CLAUDE.md ("📦 Imports at the top"): all `use` statements live in the file
  header, with only three documented exceptions (test-mod `use super::*`,
  commented trait collisions, cfg-gated bodies). None of these qualifies: `hmac.rs`
  has six functions opening with a local `use` (`sha2::{Digest, Sha256}`,
  `crate::admin::ResourceRef`/`PurgeScope`/`GroupRef`, twice `hmac::{Hmac, Mac}` +
  `sha2::Sha256`) — the whole file is already `#[cfg(feature = "crypto")]`-gated
  via `lib.rs`, so hoisting pulls nothing into a wrong scope; `planner.rs` repeats
  `use crate::filter::FilterValue;` inside three fn bodies while the header already
  imports `crate::filter::Filter` and even spells `crate::filter::FilterValue`
  fully-qualified elsewhere (line 148) — three styles for one import in one file;
  `batch_op.rs:260` imports `QueryValue`/`Value` inside `deserialize`;
  `table_ref.rs:52` imports `serde::de` inside `deserialize`.
- Failure scenario: Rule erosion — the CLAUDE.md rule exists specifically because
  mid-body imports were a repeated violation; each unjustified instance normalizes
  the next. The planner.rs triple also misleads readers about what the file
  imports.
- Suggested fix: Hoist all ten to the file headers (`hmac.rs`: one `use` block for
  `hmac::{Hmac, Mac}`, `sha2::{Digest, Sha256}`, `crate::admin::{GroupRef,
  PurgeScope, ResourceRef}`; `planner.rs`: add `FilterValue` to the existing
  `use crate::filter::Filter;` line and delete the three local copies plus the
  fully-qualified spellings).

### 7.4 — low — Mid-function `use` statements in standalone test files
- File:line: `src/batch/tests/planner_tests.rs:456,531,583,1050`;
  `src/read/tests/query_record_tests.rs:78,102`;
  `src/filter/tests/filter_value_conv_tests.rs:113,123`;
  `src/wire/tests/repl_tests.rs:22,32`;
  `src/read/tests/pagination_after_tests.rs:120`;
  `src/write/tests/insert_op_tests.rs:25`.
- Issue: The imports-at-top rule's test exception covers only `use super::*`-style
  imports *inside an inline `#[cfg(test)] mod tests` block* — these are separate
  test files, whose imports belong in the file header. Worst case is
  `planner_tests.rs`: its header (line 8) already imports
  `crate::filter::{Cond, Filter, FilterValue}`, yet test functions at lines 456 and
  531 locally re-import `Filter`/`Cond` — a shadowing re-import that misleads
  (exactly the "mislead" outcome the exception clause guards against, inverted).
  The others (`ByteBuf` imported in two tests of the same file, `QueryValue` in two
  helpers of `repl_tests.rs`, `mpack`/`RecordId`/`new_map`/`TSet` once each) are
  trivially hoistable with no collision.
- Failure scenario: Reader scanning `planner_tests.rs`'s header concludes
  `Cond`/`Filter` are imported once; the duplicate local imports rot independently
  if the header import is later narrowed.
- Suggested fix: Delete the local `use`s and extend the file-header imports; in
  `planner_tests.rs` lines 456/531 the imports are already present at the top and
  can simply be deleted.

### 7.5 — low — `FieldPath` type alias defined in `filter/mod.rs`
- File:line: `crates/shamir-query-types/src/filter/mod.rs:19-21` (verified in
  source).
- Issue: `pub type FieldPath = Vec<String>;` is a type definition living in a
  `mod.rs` that otherwise correctly contains only `pub mod`/`pub use` declarations.
  The re-export-only rule says types live in sibling files; this alias is consumed
  crate-wide (`crate::filter::FieldPath` in validator, read, filter modules), so it
  is a real export with a real definition, not wiring.
- Failure scenario: same blame/template-erosion argument as 7.1, at alias scale.
- Suggested fix: Move the alias (with its doc comment) to a sibling file (e.g.
  `filter/field_path.rs`) and re-export it: `pub use field_path::FieldPath;`. All
  existing `crate::filter::FieldPath` paths remain valid.

### 7.6 — low — `is_false` helper defined four times with three visibilities and two referencing conventions
- File:line: `src/admin/types/db_ops.rs:6`; `src/admin/types/schema_ops.rs:160-164`;
  `src/admin/types/repl_ops.rs:36-45`; `src/read/read_query.rs:52-54`.
- Issue: The identical one-line serde helper exists as: `pub(crate) fn is_false` in
  `db_ops.rs` (the de-facto shared copy, imported by six sibling files plus
  `auth/types.rs`), `pub fn is_false` in `schema_ops.rs` (referenced via
  fully-qualified serde attribute strings at lines 64/144/155), a documented
  "declared locally to keep this module self-contained" `pub(crate)` copy in
  `repl_ops.rs`, and a private copy in `read_query.rs`. Three visibilities, two
  referencing styles, one trivial function. (The `default_repo()` helper repeated
  privately across eight files is the conventional serde-default-fn pattern and
  acceptable; `is_false` is not, because three of the four copies are explicitly
  shared/cross-referenced.)
- Failure scenario: A behavioral tweak to one copy (e.g. also skipping `true` for a
  new sentinel mode) silently diverges the wire shape per module family.
- Suggested fix: Keep exactly one `pub(crate) fn is_false` (a neutral home such as
  the crate root or a small `serde_helpers` sibling), import it everywhere, and
  delete the other three copies.

### 7.7 — nit — *(dedup: folded into the 4.17 composite doc-drift finding)* Duplicated `fk_restrict` entry in `DbResponse::Error` doc vocabulary
- File:line: `crates/shamir-query-types/src/wire/db_message.rs:330-332`.
- Style-lens framing (folded into 4.17): the doc comment names `fk_restrict` twice
  (lines 330 and 332) — copy-paste drift in the list developers use as the
  authoritative error-code enumeration. Fix: delete the duplicate token.

### 7.8 — nit — *(dedup: folded into the 4.17 composite doc-drift finding)* `hmac.rs` module doc: second half of the canonical-input table is an orphaned headerless block
- File:line: `crates/shamir-query-types/src/hmac.rs:24-68`.
- Style-lens framing (folded into 4.17): the "# Per-op canonical input" table
  (header + 13 rows, lines 28-42) is interrupted by three prose paragraphs (44-59),
  then eight MORE pipe-rows (create_group … create_scram_user, 61-68) with no
  header — Markdown will not re-join them, so the documented canonical inputs for
  all group/function/superuser/SCRAM ops render as literal pipe text. A dev
  grepping rendered docs for the `create_scram_user` canonical form may misread the
  null-byte layout of an HMAC-gated destructive op. Fix: restart the table (repeat
  the header after the prose) or move the prose below a single table.

### 7.9 — nit — Inconsistent `//!` module-doc headers
- File:line: `src/subscribe/deliver_mode.rs:1`, `event_mask.rs:1`, `source.rs:1`,
  `subscribe_op.rs:1`, `unsubscribe_op.rs:1`; also `src/tests/hmac_tests.rs:1`,
  `src/validator/tests/write_op_tests.rs:1`, `src/wire/tests/db_message_tests.rs:1`.
- Issue: Nearly every implementation file in the crate opens with a `//!` purpose
  header (read/, batch/, wire/, admin/, write/, filter/ all do, including
  one-liners like `fk_action.rs`); the entire `subscribe/` module and several test
  files start with a bare `use` instead. Within-subscribe mod.rs likewise has no
  module doc, unlike its peers.
- Failure scenario: purely documentary — inconsistent file headers make grep- and
  convention-driven navigation less reliable.
- Suggested fix: Add one-line `//!` headers (e.g. `//! [`DeliverMode`] — how
  matching events are delivered to the subscriber.`) matching the crate's
  established pattern.

### 7.10 — nit — Inconsistent per-file granularity: `types.rs` multi-type buckets vs. per-family splits
- File:line: `src/write/types.rs:17-172`; `src/auth/types.rs:14-245`.
- Issue: "One file = one primary export … closely-coupled group" is applied
  unevenly. The same crate that gives `admin/types/` fourteen per-family files
  (db_ops, table_ops, index_ops, …) and splits `write/` into single-type files
  (`inserted_record.rs`, `write_result.rs`) lumps eight public DML types plus three
  select-config types into `write/types.rs` and ten public auth types into
  `auth/types.rs`. The families are defensible as "closely coupled", but the
  generic `types.rs` bucket names hide the split points the admin layout makes
  explicit, and per-op diffs are less atomic than the sibling convention. (Related
  micro-inconsistency: `#[cfg(test)] mod tests;` sits before the re-exports in
  wire/batch/filter/write mod.rs but after them in read/subscribe/admin-types
  mod.rs, and `lib.rs` places its four `pub use` re-exports at the bottom, after
  `mod tests;`, unlike every mod.rs header.)
- Failure scenario: per-op diffs touch multi-type bucket files; the convention's
  atomic-diff goal degrades gradually.
- Suggested fix: Next time either file is materially touched, split along the
  family seams already proven in `admin/types/` (e.g. `write/insert_op.rs`,
  `write/update_op.rs`, `write/select_configs.rs`); no urgent action required.

---

## Finding counts

Raw lens-tagged total: **66** (0 critical · 7 high · 21 medium · 27 low · 11 nit),
matching the workspace SUMMARY's per-crate row exactly. After deduplication of
same-root-cause defects flagged across lenses (exemplar convention: each distinct
defect counted once, under its primary lens, with absorbing lenses noted inline
above):

| Severity | Lens-tagged findings | Distinct defects | Finding numbers (dedup groups count once; absorbed lenses in parens) |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 7 | 7 | 1.1 (absorbs is_admin half of 3.4) · 3.1 (absorbs 4.7, half of 6.3) · 4.1 (absorbs 1.3) · 4.2 · 6.1 · 7.1 · 7.2 (absorbs 1.7, 4.18) |
| medium | 21 | 15 | 1.2 (absorbs 4.4) · 1.4 (absorbs 5.5) · 1.5 · 3.2 (absorbs 5.1) · 3.3 (absorbs 5.2, half of 6.3) · 3.4 (distinct nested-HMAC half) · 3.5 (absorbs half of 4.5) · 4.3 · 4.6 · 4.8 · 4.9 · 6.2 · 6.4 · 6.5 · 7.3 |
| low | 27 | 22 | 1.6 (absorbs 5.4) · 1.8 · 3.6 (absorbs half of 4.5) · 3.7 · 3.8 · 3.9 · 4.10 · 4.11 · 4.12 · 4.13 · 4.14 · 4.15 · 4.16 · 5.3 · 5.6 · 6.6 · 6.7 · 6.8 · 6.9 · 7.4 · 7.5 · 7.6 |
| nit | 11 | 8 | 1.9 · 1.10 (absorbs 5.9) · 5.7 · 5.8 · 4.17 (absorbs 7.7, 7.8) · 6.10 · 7.9 · 7.10 |
| **total** | **66** | **52** | |

Deduplicated defect census: **0 critical, 7 high, 15 medium, 22 low, 8 nit = 52
distinct defects** (66 lens-tagged findings). 14 findings are absorbed as
duplicates/splits: 1.3→4.1 · 1.7→7.2 · 3.4 partially (is_admin half→1.1; nested-HMAC
half counted as its own medium) · 4.4→1.2 · 4.5 split→3.5+3.6 · 4.7→3.1 ·
4.18→7.2 · 5.1→3.2 · 5.2→3.3 · 5.4→1.6 · 5.5→1.4 · 5.9→1.10 · 6.3 split→3.1+3.3 ·
7.7, 7.8→4.17. Kept separate despite touching the same code (different root causes,
cross-noted inline): 3.8 vs 4.8 (clamp vs serde-default on `BatchLimits`) · 6.7 vs
3.2 (walk redundancy vs walk unboundedness) · 6.8 vs 5.7 (double-encode vs
panic-on-encode in `Pagination::eq`) · 4.2 vs 6.4 (u64 coercion vs decode cost in
`FilterValue`) · 6.1 vs 4.3 (codec cost vs dispatch ambiguity in `BatchOp`) ·
3.7 vs 4.16 (which ops gated vs how required-ness is typed).

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Depth-bound the decode (the headline DoS).** Add a counting
   `Deserializer` wrapper (hard fail ~128 nested containers) at every untrusted
   ingress for `Filter`/`FilterValue`/`Cond`/`QueryValue`/`BatchOp` (or bounded
   manual visitors for the recursive spine; `BatchOp::deserialize` can pre-pass
   depth over its buffered `QueryValue`), plus a transport-level max-frame check;
   correct `MAX_FILTER_DEPTH`'s doc to say it bounds post-parse walks only. Red
   test: a few-KB ~10⁵-deep frame must return `Err`, not abort. Closes **3.1**
   (and **4.7**, the decode half of **6.3**).
2. **Depth-bound the post-parse walks.** Convert `detect_cycle`'s DFS and
   `calculate_max_depth` to iterative worklists (or cap chain length mirroring
   `NESTING_WALK_LIMIT`; `topological_sort` is the in-file pattern) and give the
   `extract_deps_*` walkers an explicit capped depth parameter. Red test: a linear
   N-entry chain under a high operator `max_queries_per_batch` must error
   (`TooDeep`), not abort. Closes **3.2/5.1**.
3. **Fix the gate classification.** Make `is_admin` an exhaustive `match`
   including `ForEach` (decide `Call`/`Subscribe`/`Unsubscribe` explicitly,
   mirroring `is_write`); add a recursive `collect_destructive_ops` helper
   (`collect_required_access` shape) and drive the server's coarse superuser gates
   AND `check_destructive_hmacs` through it so nested `Batch`/`ForEach` bodies are
   gate-checked and HMAC-tagged. Red tests first: `for_each_is_admin_reflects_body`
   (mirroring `nested_batch_is_admin` / `for_each_is_write_reflects_body`) and
   "nested `DropDb` requires its tag". Coordinate with shamir-server. Closes
   **1.1** and the distinct half of **3.4**.
4. **Close the `$cond` depth-check hole.** Extend `check_filter_depth` to descend
   `FilterValue` operands (Cond/Expr/FnCall/Array) in the same iterative walk (or
   fix the doc to drop the `$cond` claim), and add the `$cond`-embedded over-deep
   test. Closes **3.3/5.2** and the guard half of **6.3**.

**P1 — soon**

5. **Restore `InsertedRecord._id` on deserialize** (extract in `visit_map` or
   fall back to `fields.get("_id")` in `get_value_owned`), fix the stale doc, and
   add the round-trip accessor Red test. Closes **4.1/1.3**.
6. **Fix `FilterValue`'s u64 contract**: add `UInt(u64)`/`Big` before `Float` (or a
   strict-integer `Float`) mirroring `QueryRecord`'s lossless promotion, and wire
   the hand-written key-dispatch deserializer from item 15's pattern to also kill
   the untagged buffering cost. Red test: `u64::MAX` equality filter round-trip.
   Closes **4.2** (and **6.4** if the dispatch lands together).
7. **Base58 every wire-visible `RecordId`** (`QueryResult::op_id`,
   `DdlOpStatus.op_id`, `RenameIndexOp.request_id`) via the existing
   `id_as_base58_string`/`opt_record_id_base58` helpers, with wire-shape tests
   asserting `QueryValue::Str`. Closes **1.2/4.4**.
8. **HMAC canonical-input hygiene**: include `cascade`/`dst_path` (and
   `if_exists`/`replace`) in canonical forms with a `hmac key v1`→`v2`
   domain-separation bump; reject interior-NUL/empty components (or length-prefix
   parts); add the byte-equality tests for the four untested `canonical_*` helpers
   and fix the trailing-`\0` doc drift; add the `requires_hmac()` exhaustive
   classifier for the ungated op families. Closes **3.5**, **3.6**, **3.7**,
   **1.8**, and **4.5**.
9. **`BatchLimits`: defaults + clamping.** `#[serde(default)]` all six fields
   (the #662 pattern) and add `BatchLimits::clamped_against(&server_caps)` for the
   server to clamp all six through one call site; rename the doc to
   "client-requested limits (server clamps)". Closes **4.8** and **3.8**.
10. **Redact `ChangePasswordVerify`'s `Debug`** (manual impl or redacting newtype
    for `new_stored_key`/`new_server_key`/`client_proof_old`). Closes **3.9**.
11. **Pin the dispatch invariant**: a generated/walk-all-ops test asserting each op
    struct's field names intersect the discriminator list only at its own
    discriminator (or move to an explicit single-key envelope long-term). Closes
    **4.3**.
12. **Test debt in this crate's own scope**: port the stranded planner/
    reference/`PaginationInfo::compute`/`collect_required_access` suites, the four
    headline `BatchError` variants, and the NaN/inf `QueryRecord` decode test;
    replace the vacuous `fts_default_mode_is_and` with a wire-level defaulting
    test; add the 64/65 `check_filter_depth` boundary test; make `TableRef` reject
    trailing seq elements with a small `table_ref` test file. Closes **1.4/5.5**,
    **1.5**, **1.9**, **1.10/5.9**.
13. **Structural conformance**: move `WriteOp`/`ValidationError`/`CallOp` out of
    `mod.rs` (tests already split along the seam); move the two inline
    `#[cfg(test)]` blocks into the existing `tests/` dirs, deduplicating the
    overlapping assertions. Closes **7.1**, **7.2** (and **1.7**, **4.18**).
14. **Saturating pagination math + `page: 0`**: `saturating_mul` at limit.rs:180,
    `saturating_add` at :294, reject/normalize `page == 0`, tests in `read/tests/`.
    Closes **1.6/5.4**.
15. **`BatchOp::deserialize` perf, benched**: borrow the keys (no `Vec<String>`
    clone), replace the ~75-probe `has()` chain with a static discriminator table,
    and add a `bench_scale_tool::Harness` bench (baseline first per /opti).
    Longer term, a Content-style bridge or externally-tagged op tag to drop the
    msgpack re-encode. Closes **6.1** (and the mechanical half of **4.3**).

**P2 — backlog**

16. **`InsertedRecord::serialize`**: sorted-key invariant at construction or a
    cached permutation; linear per-serialize emit; bench first. Closes **6.2**.
17. **`QueryRecord` accessor borrow**: mirror `get_value_str` for
    `get_value_{i64,u64,bool}`. Closes **6.5**.
18. **Planner allocation trims**: drop the redundant `aliases` set, drain
    `provenance` into `deps`, single `deps.iter()` pass, `Box<str>` keys if needed;
    decode plan-time markers straight from `QueryValue` (no codec) — ideally once
    6.4's single-pass `FilterValue` deserializer exists. Closes **6.6**, **6.9**.
19. **Fused classification walk**: one bounded pass computing
    (repos, required_access, has_write) with dedup — land together with the 3.2
    depth-bounding so the walkers are fixed once. Closes **6.7**.
20. **`Pagination::eq`**: element-wise comparator (or cached encoded form) that
    also removes the `key_bytes` `expect`. Closes **6.8** and **5.7**.
21. **Typed vocabularies**: convert the closed `String` vocabularies to serde
    enums and publish `pub const` error-code strings. Closes **4.6**.
22. **Wire-consistency backlog**: `.count`/`.length` reservation escape hatch
    (**4.9**); `InsertedRecord` non-map envelope (**4.10**); `QueryRecord`
    Bin/`IdBytes` alias + non-`Null` `as_value()` (**4.11**); tag-key/casing
    standardization (**4.12**); `de_field_path` on all `FieldPath` fields
    (**4.13**); `InsertOp` channel ordering doc (**4.14**); `query_version`
    coverage (**4.15**); `validate_hmacs()` helpers for `Option` HMAC fields
    (**4.16**).
23. **Error-type hygiene**: replace the `From<QueryValue>` tier-3 `Null` fallback
    with `try_from`-style API (**5.3**); adopt `thiserror` (or document the
    no-macro rationale) and type the `Result<(), String>` validators (**5.6**);
    invariant comments on the HMAC `expect`s (**5.8**).
24. **Cosmetics**: per-construction `"main"` → `Cow<'static, str>` (**6.10**);
    hoist the 10 mid-function impl-file imports and 12 test-file imports
    (**7.3**, **7.4**); relocate `FieldPath` out of `filter/mod.rs` (**7.5**);
    unify `is_false` (**7.6**); fix the composite doc/wire drifts incl. the
    `fk_restrict` duplicate, the orphaned hmac table, validator examples,
    `batch_limits` doc table, `has_next_hint`, and the `CURRENT_REPL_PROTO_VER`
    re-export (**4.17/7.7/7.8**); `//!` headers for `subscribe/` and test files
    (**7.9**); `types.rs` family splits on next touch (**7.10**).
