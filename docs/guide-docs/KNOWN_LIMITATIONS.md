# Known Limitations

S.H.A.M.I.R. is an alpha-stage database (see the [Project Status](../../README.md#-project-status)
section in the root README). This document is a single, honest,
citation-backed list of the architectural limitations known to exist in the
current codebase. Every bullet cites the source file/line (or the
already-published reference doc) that backs the claim, so the list stays
verifiable rather than aspirational.

This is not an exhaustive bug list — it covers **structural** limitations:
things the current design does not attempt to solve yet, as opposed to bugs
that will be fixed without a design change. Where a limitation has a tracked
follow-up, that is noted as "planned, see roadmap" rather than an internal
task id (task IDs are internal to the development process, not a public
artifact).

## 1. Transactions

- **One repository per transaction.** A transactional batch (or an
  interactive/open transaction) whose queries span more than one repo is
  rejected with `BatchError::CrossRepoNotSupported`. See the guard in
  `crates/shamir-engine/src/query/batch/batch_execute.rs:126-132`
  (one-shot transactional batches) and the mirrored guard in
  `crates/shamir-engine/src/query/batch/interactive_tx.rs:80-87`
  (interactive/open transactions).
- **No savepoints / nested transactions.** A transactional sub-batch (a
  `Batch` op with `transactional: true`) nested inside an already-open
  transaction is rejected (`nested_tx_not_supported`). See
  `crates/shamir-engine/src/query/batch/query_runner.rs:330-341`.
- **A WASM `Call` inside an open transaction is rejected.** A `Call`
  operation delegates to the `FunctionInvoker` with autocommit semantics —
  its writes would commit independently of the enclosing transaction,
  silently breaking atomicity. This is rejected explicitly
  (`call_in_tx_not_supported`) rather than allowed to run. See
  `crates/shamir-engine/src/query/batch/query_runner.rs:709-736`
  (guard at lines 720-727).
- **Transactional DDL is not supported.** Admin operations (create
  table/index, schema changes, etc.) are always delegated to the
  `AdminExecutor` outside of any `TxContext` — they never run inside an
  open transaction's commit pipeline, transactional or not. See
  `crates/shamir-engine/src/query/batch/query_runner.rs:697-698`
  ("Admin ops — delegate to AdminExecutor (no tx)").
- **Read-your-own-writes (RYOW), current behavior.** Streaming scans
  (`list_stream_tx`/`filter_stream_tx`) and the match-scans behind
  `execute_update_tx`/`execute_delete_tx` overlay a transaction's own
  staged `write_set` on top of the committed-store stream: a staged
  insert made earlier in the SAME transaction is visible mid-scan, a
  staged update yields the staged (new) bytes, and a staged delete is
  hidden even though the committed store still has the row. See the doc
  comments in `crates/shamir-engine/src/table/table_manager_streaming.rs:170-186`
  and the merge algorithm in `crates/shamir-engine/src/table/tx_scan_overlay.rs:1-29`.
  - **Residual limitation: no SSI predicate/range locking over streams.**
    A concurrent OTHER transaction's phantom insert into a range this
    transaction is scanning is NOT detected — full SSI predicate/range
    locking over a stream is a separate, harder problem and remains out
    of scope. See the "Streaming-scan SSI scope" doc comment at
    `crates/shamir-engine/src/table/table_manager_streaming.rs:163-168`.
  - **`AsOf`/`History` temporal reads do not get the transaction overlay.**
    `read_as_of`/`read_history` in `crates/shamir-engine/src/table/read_temporal.rs:45,209`
    take no `TxContext` parameter at all — point-in-time historical views
    are exempt from RYOW by design (they answer "what did this look like
    at version/timestamp X", not "what does the current transaction see
    right now").

## 2. Schemas

- **`default`/`auto_now`/`auto_now_add` apply to single-segment (top-level)
  field paths only.** A rule with a multi-segment path is rejected at DDL
  time with `nested_path_transform_not_supported` — the write path
  (`apply_defaults`/`apply_transforms`) only ever honors single-segment
  paths, so this DDL-time guard prevents a rule from being silently
  accepted and then silently ignored on every write. See
  `crates/shamir-db/src/shamir_db/execute/admin_schema.rs:164-199`.
- **`unique` constraint: single field only.** `unique` is a per-field
  boolean rule (`Option<bool>`); there is no composite/multi-field unique
  constraint. See
  `crates/shamir-query-types/src/admin/types/schema_ops.rs:108-112`.
- **`foreign_key`: single field, same-repo target only.** A foreign key
  references exactly one field in one parent table, and that parent table
  must live in the same repo — there is no cross-repo foreign key. See
  `ForeignKeyDto` in
  `crates/shamir-query-types/src/admin/types/schema_ops.rs:143-151`
  ("The parent table name (flat, same repo)"), and the FK semi-join
  primitive in `crates/shamir-engine/src/validator/validator_db.rs:176-215`.
  No composite FK, no deferred constraints, and no self-referential
  cascade exist either.
- **Renaming a table with a bound declarative schema is rejected.** The
  auto-bound schema validator is registered under a name that embeds the
  table path, so a rename would orphan it; the guard refuses up-front. See
  `crates/shamir-db/tests/rename_table_e2e.rs:139-144`.
- **The "migration" API changes the storage engine, not the schema.**
  `StartMigration`/`CommitMigration`/`RollbackMigration`/`MigrationStatus`
  copy a table's raw `data_store` bytes to a new backend keyed by
  `dst_engine`/`dst_repo` — this is storage-engine migration (e.g. moving
  a table to a different backend), not schema evolution (there is no
  column add/rename/drop-with-data-transform facility here). See
  `crates/shamir-db/src/shamir_db/execute/admin_migration.rs:19` (entry
  point) and the `dst_engine` resolution at lines 90-98.
- **The migration API is experimental, opt-in only, and DISABLED BY
  DEFAULT.** `StartMigration` is rejected with a structured
  `experimental_feature_disabled` error unless an operator has first
  called `ShamirDb::enable_experimental_migration_api()`; the live
  `shamir-server` never calls this, so no regular client can trigger a
  migration. This gate exists because the feature has several known,
  unfixed correctness gaps (all tracked as future work):
  - **No write interception.** `MigrationShadowLog` is constructed at
    `StartMigration` time, but nothing in production code ever appends
    to it — a write landing on the source table between the initial
    snapshot copy and the final commit is silently lost from the
    destination. `CommitMigration` only compares record COUNTS, so an
    in-place field edit (no row-count change) is not even detected as a
    discrepancy.
  - **Only `dst_engine: "in_memory"` is supported** — not useful for a
    real durability-preserving migration.
  - **Non-durable coordinator state.** `MigrationCoordinator` lives
    only in the in-memory `ShamirDb::active_migrations` map — a server
    restart mid-migration loses all state, with no recovery path.
  - **`try_lock`-based duplicate-start guard.** Lock contention (a
    legitimately concurrent, in-progress operation) is indistinguishable
    from "no migration running," so a second `StartMigration` can race
    in.
  - **No post-commit history / list-all-migrations capability.**
  See `crates/shamir-db/src/shamir_db/shamir_db/core.rs`
  (`enable_experimental_migration_api` / `experimental_migration_enabled`)
  for the gate and `crates/shamir-db/src/shamir_db/execute/admin_migration.rs`
  for the entry-point check.

## 3. Indexes

- **`unique` and `sorted` are mutually exclusive on the same index.**
  Creating an index with both `unique: true` and `sorted: true` is
  rejected. See
  `crates/shamir-query-types/src/admin/types/index_ops.rs:12-21`.
- **One vector index per table.** DDL refuses to create a second vector
  index on a table that already has one, regardless of field or
  dimension — `staged_vectors` in `TxContext` keys by table token (not
  index), and post-commit `promote_vectors` fans the same batch of
  vectors out to every vector backend on the table, so two indexes with
  different `dim` would cause a `DimMismatch` on promote. See
  `docs/guide-docs/guide/06-search.md:151-160`.
- **No partial indexes, no TTL indexes, no geo indexes.**

## 4. Subscriptions

- **Best-effort delivery; a supported subset of filter shapes only.**
  Subscription filters only support
  `Eq, Ne, Gt, Gte, Lt, Lte, In, NotIn, IsNull, IsNotNull, Exists,
  NotExists, And, Or, Not`; anything else (`like`, `ilike`, `regex`,
  `contains`, `contains_any`, `contains_all`, `between`, `field_eq`,
  `fts`, `vector_similarity`, `computed`) is rejected at grant time with
  `subscription_filter_unsupported_operator`. See §7 ("Grant rejections")
  in
  [`client-server-protocol-spec/SUBSCRIPTIONS.md`](client-server-protocol-spec/SUBSCRIPTIONS.md#7-grant-rejections).
- **No durable offsets / resume tokens.** A missed range surfaces as a
  best-effort `gap` push (§9 of the same doc); there is no
  client-presentable resume token that guarantees exactly-once replay.
- **A slow consumer can experience a gap.** When a per-connection push
  channel stays full for `SLOW_CONSUMER_THRESHOLD` consecutive attempts,
  the bridge emits a `slow_consumer` push followed by a best-effort
  `closed` and tears the subscription down — the client must reconcile
  out-of-band and re-subscribe. See
  `crates/shamir-server/src/subscriptions/push.rs:80-115`
  (guard at lines 99-115).

## 5. Replication

- **Experimental, pull-based, read-only follower.** Leader-follower
  replication is an async, single-leader read-replica feature, not a
  clustering/HA solution, and is explicitly labeled Experimental. See the
  "Leader-follower replication — реализовано (Experimental)" section and
  its limitations paragraph in
  [`guide/08-interconnect.md`](guide/08-interconnect.md).
- **A journal gap is now a terminal, visible error.** When the leader
  reports a `gap_at` past the follower's requested `from_version`, the
  follower loop stops (rather than silently skipping past the missing
  range) with `ReplError::JournalGap`, and the affected subscription is
  marked `resync_required` via `mark_subscription_resync_required` —
  visible through the existing `ReplicationStatus`/`ListSubscriptions`
  admin surface. Recovery is a manual operator step (verify/fix the
  follower's data, then issue the existing `Resume` admin action); full
  automated snapshot-based reseed remains planned, see roadmap. See
  `crates/shamir-server/src/replication/error.rs:38-57` and
  `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:563-594`.

## 6. Results

- **Query results materialize fully into a `Vec`; no true server-side
  streaming to the client yet.** `QueryResult.records` is a
  `Vec<QueryRecord>` built and returned in one shot. See
  `crates/shamir-query-types/src/read/query_result.rs:64-66`. This is now
  mitigated on the wire/client side by server-side cursors (`CreateCursor`/
  `FetchNext`/`CancelCursor`, see
  [`client-server-protocol-spec/CURSORS.md`](client-server-protocol-spec/CURSORS.md)),
  which page results so neither side holds the full set in memory over the
  wire at once — but the SERVER still executes a full pinned-version scan
  per page internally (no true server-side streaming cursor at the engine
  level), so server-side peak memory during a single page's execution is
  not reduced by cursors; only wire/client-side memory is.
- **Result-size and connection caps (current defaults).** A batch
  response is clamped to `max_result_size_bytes` (default **64 MiB**),
  and the server enforces a global `max_active_connections` cap (default
  **1000**, with a per-source-IP sub-cap default of 100). See
  `crates/shamir-server/src/config.rs:288-318` (`max_result_size_bytes`
  default) and `:330-366` (`max_active_connections`/
  `max_active_connections_per_ip` defaults).
- **Cursors only support `Temporal::Latest` reads.** `AsOf`/`History`
  queries are rejected outright at `CreateCursor` with
  `cursor_temporal_not_supported`, not silently downgraded to `Latest`. See
  `crates/shamir-server/src/db_handler/cursor_handlers.rs`'s
  `create_cursor` and
  `crates/shamir-query-types/src/batch/batch_error.rs`'s
  `BatchError::CursorTemporalNotSupported`.
- **Cursors do not support `with_version: true`.** CR-B5: `CreateCursor`
  rejects `query.with_version == true` outright with
  `cursor_with_version_not_supported`, the same "reject, don't silently
  downgrade" discipline as the `Temporal::Latest`-only scope cut above — a
  cursor's every internal read runs at a pinned `Temporal::AsOf` snapshot,
  and that read path does not attach per-record versions, so honoring the
  flag would silently produce no versions instead of the real per-record
  stamps a plain (non-cursor) `with_version: true` read returns. See
  `crates/shamir-server/src/db_handler/cursor_handlers.rs`'s
  `create_cursor` and
  `crates/shamir-query-types/src/batch/batch_error.rs`'s
  `BatchError::CursorWithVersionNotSupported`.
- **Keyset-mode cursors: `Null`/missing and `Bin`/`List`/`Dec`/`Big` `ORDER BY`
  values are handled; mixed-type `ORDER BY` values are handled for
  schema-typed scalar columns whose rule was bound while the table was
  empty (F-17 #810 tightened this from F-1 #792's original, unverified
  claim); `NaN` `ORDER BY` values are mitigated (no keyset attempt) but
  not detected (CR-D2 #783, W-2/W-3 #789, F-1 #792, F-17 #810).** A
  keyset cursor's page boundary is an inclusive `field >= seek_key` (ASC) /
  `field <= seek_key` (DESC) filter — any row whose `ORDER BY` value cannot be
  compared to `seek_key` makes that filter unresolvable (`false`), silently
  excluding the row from every page after the first with a clean `has_more:
  false` at the end (no error). Current state:
  - **`Null` / missing value — CLOSED.** `CreateCursor` now runs one cheap
    `WHERE <order_by_field> IS NULL LIMIT 1` existence probe against the
    same pinned snapshot the first page reads, before running that first
    page. If it finds any row, the WHOLE cursor is pinned to row-count-offset
    pagination from creation, instead of keyset mode — closing this case
    unconditionally. See
    `crates/shamir-server/src/db_handler/cursor_handlers.rs`'s
    `order_by_column_contains_null` and `create_cursor`.
  - **`Dec`/`Big` value — CLOSED (W-3, #789).** These have no `FilterValue`
    equivalent, so the boundary filter could not even be built — before this
    fix, every `FetchNext` past page 1 hard-errored ("cursor: keyset seek key
    has no comparable filter form") instead of silently dropping rows. Now
    detected the moment the candidate bookmark value is extracted (both at
    `CreateCursor` and at each `FetchNext` bookmark refresh): an unconvertible
    value is treated exactly like the pre-existing "no seek_key" case, and the
    cursor degrades to row-count-offset pagination for that call only. See
    `crates/shamir-server/src/db_handler/cursor_handlers.rs`'s `safe_seek_key`.
  - **`Bin`/`List` value — CLOSED (W-2, #789).** Unlike `Dec`/`Big`, these DO
    convert to a `FilterValue` (`Binary`/`Array`), so the boundary filter looked
    valid but could never match anything (`compare_values` has no comparison
    arm for `Bin`/`Bin` or `List`/`List`), silently dropping every row past page
    1 — the same failure shape as the `Null` case. Investigated extending
    `compare_values` with a real total order for these types (mirroring
    `ORDER BY`'s own sort-key machinery): `ORDER BY` does NOT actually sort
    `Bin`/`List`/`Set`/`Map` today (they land in an explicit "unsortable"
    bucket, order preserved via stable sort only), so there is no existing
    total order to reuse, and inventing one would need to change `ORDER BY`'s
    own semantics too to stay self-consistent — out of scope for this
    polish-batch task. Fixed instead via the SAME "detect as unsafe, fall back
    to the existing no-seek-key safety net" mechanism as `Dec`/`Big` (see
    `safe_seek_key`) — narrower than a full total-order fix, but fully closes
    the SILENT ROW LOSS (converts it into the documented, understood
    offset-mode degradation instead). **F-18 (#811) additionally excludes
    `Bin` from the schema-typed-scalar gate's accepted `TypeTag` set itself**
    (`Int`/`Bool`/`String` only, `List` was already excluded there as a
    container type) — since `compare_values` never had a `(Bin, Bin)` arm, a
    `Bin` column could never actually benefit from `PaginationMode::Keyset`;
    it only paid for the null-probe read before degrading to the fallback
    anyway. A schema-typed `Bin` column now pins `PaginationMode::Offset`
    from `create_cursor` time, same as `Dec`/`Big`/`List` — `safe_seek_key`'s
    per-value `Bin` exclusion above is no longer reachable for it (kept in
    the code as defense in depth; still exercised as-is by `List`, `Dec`, and
    `Big`, none of which changed).
  - **Mixed `QueryValue` type in one `ORDER BY` column (e.g. some rows
    `Int`, some `Str`) — CLOSED for schema-typed scalar columns whose rule
    was bound while the table was empty (F-1 #792, tightened by F-17
    #810); STILL OPEN for schemaless columns and for a schema declared
    onto an already-populated table's column.** F-1 (#792) added a gate
    (`order_by_column_is_schema_typed_scalar`) that trusted a bound
    schema rule's non-container scalar `TypeTag` (originally `Int`/`Bool`/
    `String`/`Bin`, narrowed by F-18 #811 to `Int`/`Bool`/`String` — see the
    `Bin`/`List` bullet above) as proof of column homogeneity — but
    `add_schema_rule`/
    `set_table_schema` (`crates/shamir-db/src/shamir_db/execute/
    admin_schema.rs`) validate a new rule's SHAPE only and never
    backfill-validate the table's EXISTING rows, so a schemaless table
    with mixed-type data that later got a schema rule bound after the
    fact would have made the F-1 gate return `true` (Keyset enabled)
    even though pre-existing rows aren't homogeneous — `compare_values`
    has no cross-type comparison arm, so those rows silently vanish from
    later pages. F-17 (#810) closes this properly: each schema rule now
    carries a server-computed `keyset_safe` proof, stamped
    `table.count().await? == 0` at the exact moment the rule is bound
    (persisted in the catalogue; preserved, not recomputed, when an
    unchanged rule is re-declared via upsert-by-path). The keyset gate
    now requires BOTH an accepted `TypeTag` AND `keyset_safe == true`.
    A schema bound BEFORE any row is written is genuinely, provably
    homogeneous (schema enforcement has covered 100% of the table's
    history) and keeps reaching real `PaginationMode::Keyset` exactly as
    before. A schema declared onto an already-populated table's column
    is NOT proven safe for that column and falls back to
    `PaginationMode::Offset`, same as a schemaless column, until that
    rule is proven safe some other way — a full retroactive
    backfill-validation scan is out of scope for this task, tracked
    separately if ever needed. See
    `crates/shamir-server/src/db_handler/cursor_handlers.rs`'s
    `order_by_column_is_schema_typed_scalar` and
    `crates/shamir-db/src/shamir_db/execute/admin_schema.rs`'s
    `stamp_keyset_safe`.
  - **`NaN` in an `F64` `ORDER BY` column — STILL OPEN, but MITIGATED (no
    keyset attempt at all), schema or not.** Checked `FieldRule::check_f64`
    (`crates/shamir-engine/src/validator/schema/field_rule.rs`): the schema
    validator's `F64` type check does NOT reject `NaN`, so schema enforcement
    cannot close this gap the way it closes the mixed-type gap above — even a
    schema-declared `F64` field can still hold a `NaN` value. `NaN`'s
    `partial_cmp` always returns `None` (`compare_values`'s `(F64, F64)`
    arm), so a `NaN`-valued row would still be silently dropped the same way
    if this column ever reached the keyset boundary-filter scheme; `NaN`
    additionally breaks the keyset tie-run counter's equality check (`f64`'s
    `PartialEq` on `NaN` is always `false`). F-1 (#792) mitigates this
    WITHOUT detecting `NaN` directly: `F64` (along with `Dec`/`Big`/
    containers/`Any`) is excluded from the schema-typed-scalar gate's
    accepted `TypeTag` set entirely, so an `F64` `ORDER BY` column —
    schema-typed or not — can no longer reach keyset mode at all. The column
    always paginates via row-count-offset instead, so the "silent loss"
    framing no longer applies to it — it never runs a `NaN` value through
    `compare_values`'s boundary-filter comparison in the first place. A real
    NaN-detection fix (a new cheap primitive, or a two-phase scan design: a
    keyset phase over comparable values plus an offset-bookmarked tail phase
    for the rest) remains explicitly out of scope. See
    `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`'s
    `nan_order_by_value_forces_offset_mode_no_loss` for the regression test
    covering this mitigation.
  - **W-7 (#789): the residual mixed-type/`NaN` gap can also DUPLICATE
    rows, not just omit them — this is now moot for `F64`/excluded-`TypeTag`
    columns.** The original W-7 finding was that if a keyset cursor over an
    incomparable-value column later fell back to row-count-offset pagination
    for some OTHER reason mid-scroll (e.g. CR-D1's tie-run-ceiling fallback),
    `state.offset` would undercount the true position in the global sorted
    order — earlier keyset pages had silently SKIPPED the incomparable rows
    via the boundary filter without ever counting them into `offset` —
    causing a resumed offset scan to re-return already-handed-out rows in
    addition to (still) omitting the incomparable ones. Since F-1 (#792), an
    `F64` column (or any column excluded from the schema-typed-scalar gate)
    never enters keyset mode in the first place, so there is no
    keyset-then-offset transition mid-scroll for `state.offset` to
    undercount across — this specific duplication mechanism cannot trigger
    for those columns anymore. It remains a theoretical concern only for a
    schema-typed `Int`/`Bool`/`String` column (F-18 #811 narrowed the
    accepted set from `Int`/`Bool`/`String`/`Bin`, see above) combined with
    some OTHER keyset→offset trigger (e.g. CR-D1's ceiling fallback), which
    is an orthogonal, already-accepted mechanism unrelated to value-type
    incomparability.
- **Corrupt-record reporting covers `read_exec.rs`'s scan paths only; two
  sibling files still silently skip corrupt records (F-10, #800).** A row
  whose value bytes fail to decode inside a scan is not aborted (a single
  corrupt row aborting an otherwise-successful query over millions of good
  rows would be worse than a documented gap): it is skipped from
  `QueryResult.records` (unchanged) and now reported via the new
  `QueryResult.corrupt_records: Vec<CorruptRecordRef>` field
  (`crates/shamir-query-types/src/read/query_result.rs`) instead of
  silently vanishing from the result count. Each entry is a `{ table,
  id }` pair — the id is still resolvable even though the value failed to
  decode, since ids are read independently of the value payload. The
  field is omitted from the wire when empty
  (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`), so an old
  peer that doesn't know about it never observes it, and a new peer
  reading an old response gets an empty `Vec` by default. Coverage as of
  F-10: all ~14 decode-failure sites in
  `crates/shamir-engine/src/table/read_exec.rs` (`read_collecting`,
  `read_counting`, `read_streaming`, the index2 fast path, and the
  filtered-vector-scan / `merge_staged_filtered` /
  `build_filtered_vector_result` trio) populate `corrupt_records`. **Two
  sibling files were explicitly left out of scope and still skip corrupt
  records without reporting them**: `crates/shamir-engine/src/table/
  table_manager_index_mgmt.rs` and `crates/shamir-engine/src/table/
  table_manager_streaming.rs` (same `Err(_) => continue` pattern, not yet
  wired into `corrupt_records` — tracked as follow-up work). Also not
  wired: `try_project_page_only_bytes`'s LIMIT push-down fallback in
  `read_exec.rs` itself is a free function with no `QueryResult` in scope
  to attach a corrupt entry to, and its two callers
  (`crates/shamir-engine/src/table/read_index_scan.rs`,
  `crates/shamir-engine/src/table/read_temporal.rs`) are themselves out of
  this task's scope — see the comment at that call site for detail.
- **`SelectItem::Expression` (computed SELECT fields) is REJECTED at
  execution time, not silently ignored (F-26, #819, closed).** The variant
  is accepted at every layer of the contract — wire DTO
  (`crates/shamir-query-types/src/read/select_expr.rs`), parser
  (`crates/shamir-engine/src/query/read/parser.rs`), and the public TS type
  (`crates/shamir-client-ts/src/core/types/query.ts`) — but no evaluator
  exists yet. Before F-26, `SelectProjection::new`
  (`crates/shamir-engine/src/query/read/select_projection.rs`) silently
  dropped the item from the projected output via a bare `_ => {}` catch-all
  — a syntactically valid query with a computed field in its `SELECT`
  returned a result set with that field simply absent, no error. Fixed by
  rejecting the variant with a typed `select_expression_not_supported`
  error at the two production choke points every read plan funnels
  through: `SelectProjection::new` (full scan, index2, temporal, and
  cursor read plans — a cursor's every internal read routes through the
  temporal AsOf path) and `validate_aggregate_select`
  (`crates/shamir-engine/src/query/read/aggregate.rs`, covering GROUP BY /
  aggregate-all queries). The wire shape is kept intact for a future real
  expression-evaluator implementation — only execution-time rejection was
  added.

## 7. Numbers

- **`u64` → `Big` promotion contract.** A `u64` value greater than
  `i64::MAX` promotes losslessly to `Value::Big`/`QueryValue::Big`
  instead of silently wrapping or clamping. `Eq`/`Gt`/`Gte`/`Lt`/`Lte`
  filters and `ORDER BY` correctly match/cross-compare a promoted `Big`
  value, including one stored as raw `uint64` wire bytes (fixed by FG-6 —
  `FilterNode::Compare` falls back to `materialize_at` + `compare_values`'s
  `Big`↔`Str` arm when `scalar_at` can't surface the value directly, and
  `ORDER BY`'s `QvSortKey` gained a numeric `Big` variant). See
  [`client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md`](client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md)
  for the full contract.
- **Plain `Int`↔`F64` comparison is now exact (CR-D3).** A cross-type
  comparison between a plain `Int` (`i64`, no `Big` involved) and an `F64`
  previously cast the `i64` to `f64` before comparing — since `f64`'s
  52-bit mantissa cannot represent every integer above `2^53`, this could
  silently collapse distinct large `i64` values (e.g. `i64::MAX` and
  `i64::MAX - 1`) onto the same `f64`, corrupting `Eq`/`Gt`/`Gte`/`Lt`/`Lte`
  filters and `ORDER BY` over columns like nanosecond timestamps or
  63-bit ids/hashes. Fixed in both `compare_values`
  (`crates/shamir-engine/src/query/filter/resolve.rs`) and `ORDER BY`'s
  `QvSortKey` ordering (`crates/shamir-engine/src/query/read/order.rs`) via
  an exact bounds-check + `floor`/`fract` technique — no `BigInt` needed:
  `f64`'s 11-bit exponent already covers every integer magnitude up to
  `2^63` exactly at the boundaries (`i64::MIN`/`i64::MAX` bound-check
  against exact `±2^63` `f64` literals), and any in-range `f64`'s
  `floor()` is losslessly castable to `i64`, so the residual tie only
  needs a `fract()` check. `Int`↔`Int` and `Int`↔`Dec` were already exact
  and remain unaffected; `Big`↔`F64` remains a deliberate, accepted
  approximation (see the FG-6/CR-C5 entry above) since `F64` is itself an
  inherently imprecise column type there.

## 8. `ttl_ms`

- **`ttl_ms` governs the in-memory write-back buffer, not
  data-expiration.** `MemBufferConfig::ttl_ms` (in
  `crates/shamir-storage/src/storage_membuffer.rs:104-105`) controls how
  long an entry stays in the RAM cache in front of the durable backend
  before `moka`'s eviction listener flushes it to the inner store — see
  the durability/eviction contract documented at
  `crates/shamir-storage/src/storage_membuffer.rs:48-60`. It is **not** a
  data-expiration/TTL-eviction feature: there is no automatic deletion of
  "expired" records: an evicted entry is written to the durable backend,
  never dropped.

## 9. Backup / Restore durability

- **fsync is applied to copied files, the manifest, and each rename's
  containing directory — but directory-fsync is a no-op on non-unix
  (Windows), and true power-loss crash recovery is not unit-tested (F-12,
  #802).** `backup::copy_dir_recursive` now re-opens each just-copied
  destination file and calls `sync_all()` (propagating a failure as
  `BackupError::Io`/`RestoreError::Io` — content durability is the point),
  and `write_manifest` uses `File::create` + `write_all` + `sync_all`
  instead of a bare `fs::write`, so the manifest is durable before
  `backup()` returns success. `restore()`'s step-5 atomic swap calls a
  new `fsync_dir` helper on the containing `parent` directory after EACH
  successful `fs::rename` (both swap-path renames, the rollback rename,
  and the fresh-target single rename) — this closes the classic "you
  fsync'd the file but not the directory" gap on ext4/xfs, where a bare
  `fs::rename` can return success before the directory-entry update is on
  stable storage. Directory-fsync failures are logged but NOT propagated
  (matching `shamir-wal`'s `wal_segment.rs::fsync_parent_dir` precedent:
  a missing dir-fsync degrades the power-loss window but does not corrupt
  data). **Windows / non-unix: `fsync_dir` is a documented no-op** — this
  workspace already decided (in `wal_segment.rs`'s `#[cfg(not(unix))]`
  stub) that Windows does not need this specific directory-entry
  durability guarantee; that rationale carries into backup/restore
  unchanged. **True power-loss crash injection is outside what a portable
  unit test can exercise** (there is no way to truncate the OS page cache
  mid-syscall from a `#[test]`); the regression guard is the
  interrupted-copy test in `crates/shamir-server/src/tests/
  restore_tests.rs` (`copy_step_failure_propagates_and_leaves_data_dir_
  untouched`), which verifies the closest portable proxy: a failure
  during `restore()`'s copy step propagates cleanly and leaves `data_dir`
  completely untouched. See `crates/shamir-server/src/restore.rs`'s
  `fsync_dir` and `crates/shamir-server/src/backup.rs`'s
  `copy_dir_recursive`/`write_manifest`.
