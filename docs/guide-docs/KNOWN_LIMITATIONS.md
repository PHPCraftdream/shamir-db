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
- **`CreateCursor`/`FetchNext` do not yet reject `with_version: true`.**
  Combining cursor pagination with per-record CAS version stamps
  (`ReadQuery::with_version`) is unsupported/unverified today and may
  produce confusing results; avoid combining the two until this is
  explicitly rejected or supported (tracked separately).
- **A cursor's "stable snapshot" can still be disturbed by a concurrent
  DELETE.** A cursor pins an MVCC snapshot version at `CreateCursor` time,
  but each `FetchNext` re-enumerates the table's CURRENT id set (not a
  truly frozen id set) before reading each matched id at the pinned
  version — a row deleted between two `FetchNext` calls stops being
  enumerated at all, so it silently disappears from the cursor's remaining
  pages (and, on the no-ORDER-BY offset-bookmark path, shifts subsequent
  offsets). Tracked separately as a hardening item.

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
