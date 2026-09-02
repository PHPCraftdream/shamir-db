# shamir-engine — Task groups (actionable regrouping of the 2026-08-14 7-lens review)

This document turns the synthesized `SUMMARY.md` (79 distinct defects, deduplicated from
87 lens-tagged findings across `correctness-tdd.md`, `concurrency-lockfree.md`,
`security-crypto.md`, `performance-hotpath.md`, `api-wire-protocol.md`,
`error-handling-lifecycle.md`, `style-claude-md.md`) into **31 engineering task groups**,
each sized so one engineer can pick it up and finish it in one sitting. Groups are formed by
**root cause or fix location**, not by the lens that flagged them and not by the raw
P0/P1/P2 bucket alone; a group's priority is the highest priority of any item folded into it.

**Coverage:** every one of the 79 distinct defects in `SUMMARY.md` is assigned to exactly one
group below (cross-references in parentheses do not double-count). Counted by SUMMARY.md
distinct-defect ids: Section 1 primaries (1.1, 1.2, 1.4, 1.6) = 4; Section 2 = 6;
Section 3 = 7; Section 4 = 28 numbered + 7 nits in 4.30 = 35; Section 5 = 7; Section 6 = 12;
Section 7 = 8 → 79.

**Verification basis (2026-09-02):** `git log -- crates/shamir-engine` shows no commit since
2026-08-13 (the last two, #1110 and #1111, predate the review), so the crate is unchanged since
the audit. Every P0/P1 item and every HIGH-severity finding was re-opened against the current
source (file:line and mechanism); LOW/nit items were spot-read. All checked out as real and
correctly located, with the citation nits listed in "Spot-check notes" at the end. Read-only
pass: no source edits, no build/test/lint run.

Priority legend: **P0** — should block anything else shipping from this crate;
**P1** — soon; **P2** — backlog.

---

## P0

### 1. Fix drainer Phase B silent data drop for tables that lack an `MvccStore` (and its false comment)

- **Priority:** P0
- **Description:** `Drainer::drain_step` Phase A skips *every* `WalOpV2::Put | Delete`
  (`crates/shamir-engine/src/tx/drainer.rs:421-423`), and Phase B only writes history for
  tables present in `per_table_mvcc` (`:504-516`); a table without an entry gets nothing,
  under a comment (`:517-519`) claiming `replay_v2_op` already handled its data ops in Phase A —
  behavior that does not exist. Phase C then finalizes the entry anyway (`gate.mark_durable`
  at `:605`, `wal.commit` at `:611-619`), destroying the sole durable copy of a write that
  `apply_data_batch`'s `None` arm (`commit_phases.rs:606-610`) may have failed to land and that
  the `MaterializationState::Deferred` contract promised recovery would reconcile. Cold
  recovery (`recovery.rs:80-84`, `:112-117`) *does* write `data_store` for unattached tables,
  so warm and cold paths diverge. Production reachability is the `remove_table` race
  (`repo_instance.rs:510` evicts the `per_table_mvcc` entry while earlier WAL entries for that
  token can still be in the drain window) — see spot-check note 1 — but the outcome is silent,
  permanent data loss and no test covers it (all `tx/tests/drainer_tests.rs` fixtures attach
  an `MvccStore` via `repo.get_table`).
- **Work:** in Phase B, when `read_sync` returns `None`, route the batch through
  `replay_v2_op` (or add the table to `failed_tables` so Phase C refuses to finalize); rewrite
  the Phase B comment; add a Red `drain_step` test with a hand-built entry whose `Put` targets
  a token absent from `per_table_mvcc`.
- **Source findings:** SUMMARY.md 1.2; correctness-tdd.md #2; SUMMARY.md Fix Plan P0 #1;
  (coverage gap named in correctness-tdd.md "Coverage verdict").

### 2. Fail closed on write-path pre-read errors (`delete` / `set` / `update_tx` `.ok()`)

- **Priority:** P0
- **Description:** Three pre-reads flatten every error to "record absent":
  `table_manager_crud.rs:431` (`delete_returning_version`), `:511` (`set_returning_version`),
  `table_manager_tx_ops.rs:1075` (`update_tx`). `get()` returns `Err(Codec)` for a corrupt
  stored record and `Err(Storage)` for I/O failure (`table_manager_crud.rs:591-601`), so a
  transient fault turns a delete into a silent no-op `(false, 0)` and turns set/update into the
  insert branch: `plan_insert_ops` instead of `plan_update_ops` (old unique posting never
  released — permanently blocks that value), `counter_delta = +1` on an existing row, and for
  `update_tx` the commit-time `rederive_stale_value_ops_post_stage` repair only runs when the
  repo-busy gate fires (`pre_commit.rs:1969-1977`), so a quiet repo commits the wrong plan.
  The crate already fixed this exact class on the sibling `delete_tx` path (F-65/#891,
  `TEST_READ_ONE_TX_BYTES_FAILURE` seam in `table_manager_streaming.rs`).
- **Work:** replace each `.ok()` with a `match` mapping `Err(DbError::NotFound(_))` → `None`
  and propagating everything else via `?` (mirror `read_pre_tx_bytes` /
  `recovery_marker.rs::load_u64`); one Red test per site using the existing injection seams.
- **Source findings:** SUMMARY.md 6.1 (= 1.3); error-handling-lifecycle.md #1;
  correctness-tdd.md #3; error-handling-lifecycle.md #9 item (a) (SUMMARY.md 6.9 is counted
  here; items (b)/(c)/(d) are cross-referenced from groups 15, 14, 8); SUMMARY.md Fix Plan
  P0 #2.

### 3. Drop DashMap shard guards before `.await` in `DbInstance`

- **Priority:** P0
- **Description:** `DbInstance::get_table` (`db_instance/db_instance.rs:61-68`) and all seven
  index-routing methods (`:172-184`, `:187-200`, `:203-214`, `:217-228`, `:231-242`,
  `:245-256`, `:259-271`) hold the `dashmap::Ref` (a synchronous shard `RwLock` read guard)
  across the delegated `.await`. A cold `get_table` runs `TableManager::create` (store opens,
  index loads, possible repair) and `create_index`/`drop_index` run an entire online backfill
  under that guard; any concurrent `add_repo`/`remove_repo`/`rename_repo` then blocks its
  tokio worker thread on the shard write lock, and a handful of such callers wedges the
  runtime. This is verbatim the deadlock class the crate documents and fixed in
  `RepoInstance::get_table` (`repo/repo_instance.rs:311-320`).
- **Work:** clone the `RepoInstance` out of the map and drop the guard before awaiting —
  exactly what `get_repo` (`:109-111`) already does — at all eight sites.
- **Source findings:** SUMMARY.md 2.1; concurrency-lockfree.md #1; SUMMARY.md Fix Plan P0 #3.

### 4. Bound `StoreChangelog::range_from` to `limit` instead of buffering the whole journal tail

- **Priority:** P0
- **Description:** `repo/changelog_store.rs:37-55` drains every key `>= from_key` into a
  `Vec`, sorts it, then truncates to `limit`; `batch = limit.clamp(1, 1024)` only sizes stream
  chunks. This is the durable backend for `RepoInstance::read_changelog_from` (follower
  catch-up, late-subscriber resync, admin retention), and the journal has no retention, so
  each `limit = 10` poll costs O(N) RAM and O(N log N) CPU over the entire commit history —
  sustained unbounded allocation under a replication loop.
- **Work:** stop consuming once `pairs.len() >= limit + batch` (slack for the defensive sort)
  or take the first `limit` pairs batch-wise (keys are big-endian commit versions, ascending on
  disk backends); regression test asserting bounded reads via a counting `Store` double.
- **Source findings:** SUMMARY.md 1.1 (= 4.4); correctness-tdd.md #1;
  performance-hotpath.md #4; SUMMARY.md Fix Plan P0 #4.

### 5. One wire grammar for queries: retire or align the legacy hand-written parser

- **Priority:** P0
- **Description:** The exported `query_from_value` (`query/read/parser.rs:14-78`, public via
  `query::read`) parses a dead dialect: it reads a top-level `"limit"` key instead of the
  canonical tagged `pagination` object that `BatchOp::Read` → `qv_to::<ReadQuery>`
  (`shamir-query-types/src/batch/batch_op.rs:288`) and the builder produce, cannot express
  keyset `After`, and hardcodes `temporal: Latest, with_version: false, explain: false`
  (`:74-76`) — a builder-shaped `.limit(20)` query fed through it reads the **entire table**.
  Its helper `pagination_from_value` (`query/common/parser.rs:576-606`) coerces invalid input
  (`"10"` → no limit, `-1` → `u64::MAX`, non-Int `page` silently falls through, mislabeled
  `limit.page_size` error), `order_by_from_value` silently drops unknown `nulls`
  (`:536-544`), and `table/tests/filter_stream_tests.rs` keeps the dialect alive with 27
  `filter_from_value` sites against the project's builder-only rule.
- **Work:** delete the parser family or reduce `query_from_value` to a serde round-trip; if
  kept temporarily, `#[doc(hidden)] #[deprecated]` it, turn every coercion into a coded
  error, fix the `nulls` arm, add a differential builder-round-trip test, and migrate
  `filter_stream_tests.rs` to `shamir_query_builder::filter`.
- **Source findings:** SUMMARY.md 5.1, 5.2, 5.4, 5.5; api-wire-protocol.md #1, #2, #4, #5;
  SUMMARY.md Fix Plan P0 #5.

---

## P1

### 6. Remove the O(N²·K) re-planning scans from `rederive_stale_value_ops_post_stage`

- **Priority:** P1
- **Description:** `tx/pre_commit.rs:1999-2329`: for every staged `Remove`, the code rebuilds
  `staged_removals_by_rid` by re-iterating the whole `tx.index_write_set` and cloning every
  `RemovePosting.key` (`:2014-2030`); for every re-planned op it runs a linear
  `.filter(..).any(..)` rescan of the same set (`:2089-2101` DELETE, `:2209-2300` UPDATE).
  The gate (`:1970`, `version_allocation_high_water_mark > snapshot_version`) fires under any
  concurrent write traffic, so a bulk DELETE/UPDATE of N indexed rows under load pays
  O(N²·K) inside the locked pre-commit validate phase — the exact class #1099/#1108/#1111
  already fixed next door in `released_unique_cache`.
- **Work:** build `staged_removals_by_rid` and a `TFxSet` of staged regular posting keys once
  per table before the row loop and update them as ops are appended; replace both `.any()`
  rescans with O(1) lookups; assert via the `p1107_stale_value_gate` bench.
- **Source findings:** SUMMARY.md 4.1 (= 1.5); performance-hotpath.md #1;
  correctness-tdd.md #5; SUMMARY.md Fix Plan P1 #6 (part).

### 7. Replace `snapshot_ops()` clone-and-scan staged-overlay probes with borrowed iteration

- **Priority:** P1
- **Description:** `StagingStore::snapshot_ops()` (`shamir-tx/src/staging_store.rs:172-180`)
  materializes a fresh `Vec<KvOp>` cloning every staged key and value; `ValidatorDb`
  calls it once per unique/FK schema-rule probe — i.e. per record validated —
  (`validator/validator_db.rs:312` `staged_field_matches`, `:427-440` `exists_in_self`
  step 3), and the same helper shape exists in `fk_restrict.rs`/`fk_on_update.rs`
  (`staged_field_matches`) and in `pre_commit.rs:1993`. A batch insert of M rows into a
  table with a `unique`/FK rule in one tx is O(M²) time and O(M) transient allocation per
  record on the pre-commit hot path.
- **Work:** add a non-materializing borrowed iterator (`for_each_op` / `iter_ops`) to
  `StagingStore` and switch every probe to it; optionally keep a per-table
  `(field id, scalar)` staged-value set for O(1) probes.
- **Source findings:** SUMMARY.md 4.2; performance-hotpath.md #2; SUMMARY.md Fix Plan
  P1 #6 (part).

### 8. Collapse FK fan-out scans to one deduped pass per child table, and fail closed on decode-corrupt rows

- **Priority:** P1
- **Description:** FK RESTRICT runs `child_has_reference` once per parent value
  (`query/batch/fk_restrict.rs:145-164`), each a full `list_stream_tx` child scan when no
  single-field index covers the FK column (`:373-391`); `collect_parent_values` says
  "distinct values" but pushes every matched row's value (`:273`), so duplicates multiply
  identical scans. ON UPDATE RESTRICT repeats the shape with un-deduped `restrict_fields`
  (`fk_on_update.rs:304-316`, gate `:337-355`), and `classify_row*` rebuilds a `RecordView`
  (plus a fallback full decode) per probe per row (`fk_actions.rs:1192-1246`,
  `fk_on_update.rs:1017-1079`). In the same collection loops, `if let Ok(view) =
  RecordView::new(&bytes)` silently drops the ref-field values of any decode-corrupt parent
  row (`fk_actions.rs:661`, `:1142`; `fk_on_update.rs:845`; `fk_restrict.rs:270`; same class
  in `validator_db.rs:98-108`) — a silently shrunk CASCADE/SET NULL/RESTRICT action set with a
  success result, the decode-error half of what F-65/#891 fixed for read errors.
- **Work:** dedupe parent values into a `TFxSet`; invert to one child pass per table testing
  each row's FK field against a coercing membership set (index fast path kept as an early-out
  per distinct value); hoist one `RecordView`/decode per row; on `RecordView::new` failure
  fall back to `InnerValue::from_bytes` and then return `Err(DbError::Codec)`; extend the
  `fk_indexed_action_read_error_tests.rs`-style injection to a decode-corrupt matched row
  (error-handling-lifecycle.md #9 item (d)).
- **Source findings:** SUMMARY.md 4.3, 4.18, 4.20, 6.4; performance-hotpath.md #3, #18, #20;
  error-handling-lifecycle.md #4; SUMMARY.md Fix Plan P1 #6 (part) and #10 (part).

### 9. Route ON UPDATE FK discovery through `fk_reverse_cache` behind the intersection gate

- **Priority:** P1
- **Description:** `discover_on_update_refs` (`query/batch/fk_on_update.rs:734-783`) does
  `repo.list_table_names()` then `resolver.resolve(..)` + `collect_fk_refs()` per table on
  **every UPDATE**, and it runs at `:188` *before* the set-fields ∩ ref-fields no-op gate at
  `:196-204`, so an update touching no FK-referenced field still pays an O(tables) schema
  walk. The delete path already migrated to `RepoInstance::fk_reverse_cache` (F-28 Step 4,
  `fk_actions.rs:1054`, `fk_restrict.rs:193-201`); the on-update path never did. While in the
  cache, `lookup_by_parent` clones the parent's `Vec<ReverseFkEntry>` (String fields) per hit
  (`repo/fk_reverse_cache.rs:419-428`).
- **Work:** filter cache entries on `on_update != NoAction` (mirror `discover_action_refs`),
  apply the cheap intersection gate first, and store `Arc<[ReverseFkEntry]>` values so warm
  hits are a pointer clone.
- **Source findings:** SUMMARY.md 4.5, 4.30 item 4; performance-hotpath.md #5, Nits #4;
  SUMMARY.md Fix Plan P1 #6 (part).

### 10. Make single-flight latches panic-safe: group-commit leader and background verify

- **Priority:** P1
- **Description:** `GroupCommit::leader_loop` (`repo/group_commit/mod.rs:89-117`) resets
  `leader_busy = false` only on its normal exit (`:112`); a panic inside `flush()` (`:100`)
  aborts the detached task, waiters get `Err("group-commit flush task dropped")`, but every
  later `run()` caller pushes its oneshot and parks forever — all subsequent `synced` commits
  on the repo hang (the durability-flush DoS the cancellation fix aimed to remove).
  `TableManager::bump_write_counter` (`table/table_manager.rs:946-969`) has the same shape:
  `verify_running` is cleared only on the normal path (`:968`), so one panic in `verify()`
  permanently disables background consistency verification with no signal. The group-commit
  fix lands in a `mod.rs` that carries the full 128-line implementation, the crate's only
  `mod.rs` with logic.
- **Work:** first, as its own `style:` commit, move the implementation to a sibling
  `group_commit.rs` (`mod.rs` keeps `mod group_commit; pub use ..`, with
  `#[allow(clippy::module_inception)]` like `table/table.rs`); then guard both latches with an
  RAII `Drop` reset (or `catch_unwind` + `log::error!`), and add tests with a panicking
  flush/verify asserting a subsequent `run()`/verify still proceeds
  (`group_commit/tests/` has no panicking-flush test today).
- **Source findings:** SUMMARY.md 1.4, 6.6, 7.1 (= 5.8); correctness-tdd.md #4;
  error-handling-lifecycle.md #6; style-claude-md.md #1; api-wire-protocol.md #8;
  SUMMARY.md Fix Plan P1 #7 and P2 #19 (part).

### 11. Harden `ValidatorRegistry`: atomic `add_binding`, inverse id→name map

- **Priority:** P1
- **Description:** `add_binding` (`validator/registry.rs:162-169`) is a check-then-act on a
  lock-free map: `entry_sync(id).and_modify(..)` (no-op while vacant) followed by
  `insert_sync(id, BTreeSet::from([table])).ok()`, which discards scc's `Err` when the key
  already exists. Two concurrent `bind_validator_as` calls for the same validator on
  different tables (`shamir-db/.../validator_management.rs:504`, unserialized) can both see
  the entry vacant and the loser's table binding is silently dropped from `bound_in`, defeating
  `drop_validator`'s still-bound refusal and persisting the incomplete set. In the same file,
  `remove` reverse-scans `name_to_id` (`:128-145`) and `unbind_all_for_table` calls the
  full-scan `name_for_id` per candidate (`:195-218`) — O(V²) — and `is_empty()` (`:237-239`)
  is an un-acked scc O(N) bucket walk next to the acked `len()` at `:231`.
- **Work:** collapse `add_binding` into one `entry_sync(*id).or_insert_with(BTreeSet::new)`
  critical section; keep a direct id→name map (or store the name alongside the artifact) so
  `name_for_id`/`remove` are O(1); add the `// O(N) ack:` annotation or an atomic mirror for
  `is_empty()`.
- **Source findings:** SUMMARY.md 2.2, 4.27, 4.30 item 6; concurrency-lockfree.md #2;
  performance-hotpath.md #27, Nits #6; SUMMARY.md Fix Plan P1 #8.

### 12. Extend the filter-depth and pattern-length guards to every client filter surface

- **Priority:** P1
- **Description:** `validate_filter_depth` (`query/batch/batch_validate.rs:78-97`) collects
  filters only from `Read.where`, `Delete.where_clause`, `Update.where_clause`; `entry.when`
  is compiled unchecked (`query_runner.rs:136-157`), `GroupBy::having` is compiled unchecked
  (`query/read/aggregate.rs:1304-1311`), and `check_filter_depth`
  (`shamir-query-types/src/filter/filter_enum.rs:219-238`) walks only `And`/`Or`/`Not`, so a
  depth-1 `Eq` whose value is a 100k-deep `$cond`/`Array` chain passes and then
  `resolve_filter_query`/`compile_filter` recurse unbounded per row → tokio worker stack
  overflow → process abort, not an `Err`. In the same compile path, `Regex::new(pattern)`
  has no length cap, and an invalid `Regex`/`Like` pattern folds to `FilterNode::False`
  (`query/filter/compile.rs:81-110`, `fts.rs:24`), so `DELETE ... WHERE NOT (regex typo)`
  compiles to `True` and deletes every row with no error.
- **Work:** include `when` and `having` in the collector; add an iterative `FilterValue`-tree
  depth walk to `check_filter_depth` (mirror `prescan_filter`'s dispatch); cap pattern length
  (e.g. 64 KiB) in the same validation pass; reject invalid regex/like with a coded
  `BatchError` instead of folding to `False`.
- **Source findings:** SUMMARY.md 3.1, 3.6; security-crypto.md #1, #6; SUMMARY.md Fix Plan
  P1 #9 (part).

### 13. Cache compiled `$cond` conditions on every eval path; make pointer-keyed caches lifetime-safe

- **Priority:** P1
- **Description:** `resolve_filter_query`'s `Cond` arm (`query/filter/resolve.rs:397-403`)
  calls `compile_filter(&cond.condition, ..)` per record whenever `ctx.cond_cache` is `None`,
  and the #643 `CondCache` is only populated by `SelectProjection::new`
  (`cond_cache.rs:1-16` admits WHERE/`when`/`for_each`/write-value callers don't) — a `$cond`
  whose condition contains `Regex`/`Like` recompiles the regex once per row scanned, inside a
  single op the #666 deadline never checkpoints. The cache type itself
  (`cond_cache.rs:27-49`, also `field_path_cache.rs`, `query_ref_cache.rs`) is keyed on the
  raw address of the `Filter`/`FilterValue` node; its doc covers only the benign
  clone-miss case, not address reuse after the tree is dropped, which would serve a stale
  compiled predicate for a different query — an invariant enforced only by comment.
- **Work:** prescan and thread a `CondCache` through the WHERE/`when`/write-value compile
  paths the way `SelectProjection::new` does (or cache the compiled node inside the
  `FilterNode` arm); wrap the key in a borrowing newtype (`CondKey<'a>(&'a Filter)`) so
  "cache outlives tree" is a compile error, or key on a tree hash.
- **Source findings:** SUMMARY.md 3.2, 3.5; security-crypto.md #2, #5; SUMMARY.md Fix Plan
  P1 #9 (part) and P2 #15 (part).

### 14. `RecordCounter`: fail closed on init errors and fix the dirty-flag race

- **Priority:** P1
- **Description:** `ensure_cache` (`table/record_counter.rs:175-178`) maps any info_store read
  error to `0` and `bincode::from_bytes(..).unwrap_or(0)` maps a corrupt blob to `0`, seeding
  `last_persisted = 0` as well, so the first `persist()` after any increment overwrites the
  previously durable count (e.g. 10 000 → 1) until a doctor `repair()`. In the same struct,
  `set()` (`:88-94`) and `persist()` (`:143-163`) do `write_through(..).await` then an
  unconditional `dirty.store(false)`; a lock-free `increment()` landing during the await
  (`fetch_add` + `dirty.store(true)`) has its mark erased, so the delta is invisible to the next
  `persist()` fast-path skip and the durable count drifts until a later increment re-dirties it.
- **Work:** distinguish `NotFound` (→ 0) from other errors (→ propagate) and never persist a
  value derived from a defaulted init (warn + skip write-back on decode failure); replace the
  boolean with a generation `AtomicU64` bumped on every mutation and clear only if the
  generation is unchanged (CAS), or compare against a re-read `cache` after the write; tests
  for both error classes and the race (error-handling-lifecycle.md #9 item (c)).
- **Source findings:** SUMMARY.md 6.3, 2.3 (= 1.9); error-handling-lifecycle.md #3;
  concurrency-lockfree.md #3; correctness-tdd.md #9; SUMMARY.md Fix Plan P1 #10 (part).

### 15. Release pessimistic locks when commit entry fails on `tx_gate()` / `repo_wal()`

- **Priority:** P1
- **Description:** `commit_tx_inner` (`tx/commit.rs:589-590`) does `let gate =
  repo.tx_gate().await?; let wal = repo.repo_wal().await?;` while every other early exit in
  the function (`:574-577`, `:580-587`, `:603-609`, and the sibling paths) calls
  `release_pessimistic_locks(&tx, repo).await` first. `TxContext` has no `Drop` for Level-3
  locks (`batch_execute.rs:66-68`), and `repo_wal()` performs real I/O on first touch
  (`create_dir_all`, `SegmentSet::open`), so a `Pessimistic` tx whose commit is the repo's
  first WAL touch and hits a disk/permissions error leaks its `locked_keys` permanently and
  younger waiters park in `lock_key` with no timeout. Mitigated today only because the wire
  layer exposes Snapshot/Serializable for interactive txs; direct engine embedders are
  exposed.
- **Work:** wrap both resolutions so the `Err` arm releases the locks before returning; add
  a fail-injection test forcing `repo_wal()` init failure on a Pessimistic tx and asserting
  the key is re-lockable (error-handling-lifecycle.md #9 item (b)).
- **Source findings:** SUMMARY.md 6.2; error-handling-lifecycle.md #2; SUMMARY.md Fix Plan
  P1 #10 (part).

### 16. Stop discarding `MvccStore`-attach error signals (split-brain and replication divergence)

- **Priority:** P1
- **Description:** `create_table_context` does `let _ =
  self.per_table_mvcc.insert_sync(token, ..)` (`repo/repo_instance.rs:399`); scc `insert`
  returns `Err` when the token is already present, and `remove_table`'s own A13 comment
  (`:496-510`) states that a stale entry at attach time is "a split-brain where committed
  transactions silently vanish" (the commit pipeline resolves the store by token through this
  map while the new `TableManager` reads through its own). `remove_table(X)` racing a
  `get_table(X)` init (`self.tables.remove` at `:486` lets a second init run) makes the second
  insert fail silently and keeps the old store. On the follower side,
  `apply_replicated` does `let _ = repo.get_table(&table_name).await;`
  (`tx/apply_replicated.rs:208`) to force the attach, but ignores every non-NotFound error,
  so an attach failure (store I/O, index recovery) falls through to the direct `base.transact`
  branch — the exact non-MVCC write the attach exists to prevent — and the bookmark advances so
  re-delivery cannot fix it.
- **Work:** on `Err` from `insert_sync`, `log::error!` and return `DbError::Internal` (fail
  the open); in `apply_replicated`, match `NotFound` → unattached branch, anything else →
  propagate (the caller must not advance the watermark; idempotent retry is supported).
- **Source findings:** SUMMARY.md 6.5, 6.8; error-handling-lifecycle.md #5, #8; SUMMARY.md
  Fix Plan P1 #10 (part).

### 17. Version the three raw-bincode persisted blobs through `MetaEnvelope`

- **Priority:** P1
- **Description:** `meta/envelope.rs` documents that every persisted `__meta__/*` payload is
  wrapped in the versioned `MetaEnvelope`, and `recovery_marker.rs`/`validator/persistence.rs`
  honor it, but `MemBufferConfig` (`table/buffer_config.rs:33,47`), the crash-recovery-critical
  migration `ShadowEntry` (`migration/shadow_log.rs:79,97,114`), and the index2 drop
  tombstone `Vec<(u32, String, Option<String>)>` (`table/table_manager_index_mgmt.rs:1388-1393`)
  are raw `bincode` 1.x — not self-describing, not versioned. The tombstone tuple already
  changed shape once (#1051); a sixth `MemBufferConfig` knob would make every table with a
  persisted config fail `buffer_config::load` on open with no version byte to dispatch on.
  `table/ddl_op_log.rs:34,90-95` shows the correct fail-closed version-byte pattern.
- **Work:** route all three through `MetaEnvelope` (key space already reserved) or prepend a
  `DDL_OP_LOG_VERSION`-style byte plus a migration shim in each reader.
- **Source findings:** SUMMARY.md 5.3; api-wire-protocol.md #3; SUMMARY.md Fix Plan P1 #11.

### 18. Gate the test-only `SessionPermissions` RBAC scaffolding out of the public API

- **Priority:** P1
- **Description:** `query/auth/mod.rs:10` unconditionally `pub use`s `SessionPermissions`
  while its only consumer `execute_batch_with_permissions` is `#[cfg(test)]`
  (`batch_execute.rs:265`) and its own doc (`session.rs:26-34`) calls it test-only
  scaffolding not wired into the live path (live access control is Shomer DAC via
  `ShamirDb::execute_as`). An embedder can construct it and believe it is the access model.
  Inside, `row_filter()` carries a dead first loop with an empty body and a "we need a
  different approach" note (`session.rs:162-170`, a wasted O(N) scan), and
  `extract_action_resource` unwraps `op.table_ref()` for all five data-op variants
  (`:238-254`), safe only by an unasserted invariant. `SecretString` is re-exported from the
  same module (`auth/mod.rs:11-14`) but unused here (harmless).
- **Work:** `#[cfg(test)]`-gate `SessionPermissions` alongside its consumer (or move it to a
  test-support module); delete the dead loop; replace the unwraps with `debug_assert!` plus a
  deny-by-default fallback resource.
- **Source findings:** SUMMARY.md 3.7 (= 1.7, absorbs 4.30 item 8); security-crypto.md #7;
  correctness-tdd.md #7; performance-hotpath.md Nits #8; SUMMARY.md Fix Plan P1 #12.

---

## P2

### 19. Hoist per-batch invariants out of insert/update row loops and cache `table_token`

- **Priority:** P2
- **Description:** `insert_tx_many` (`table/table_manager_tx_ops.rs:705-722`) and
  `insert_tx_many_bytes` (`:920-938`) call `iter_unique_indexes()` — which yields owned
  `IndexDefinition` clones — inside the per-row loop, while the non-tx sibling already hoists
  the snapshot with a flamegraph-justified comment (`table_manager_crud.rs:285-291`). The
  non-tx batched insert calls `index2_on_insert` per record (`table_manager_crud.rs:373-375`),
  each re-walking `index2_registry.all_backends()` (`:89-97`) against a set that cannot change
  mid-batch. `execute_update_tx` re-hashes the immutable table name via
  `self.table_token()` per matched row plus a 16-byte `id.to_bytes()` probe allocation
  (`table/write_exec.rs:664-667`), and `table_token()` itself is a fresh SipHash over the name
  on every call (`table_manager.rs:15-20`, `:992-994`), 2-5× per write op.
- **Work:** hoist `unique_defs` and `backends` above the row loops; compute the table token
  once at construction and store the `u64`; hoist `token`/`staging` above the update loop and
  probe with `id.as_bytes()`.
- **Source findings:** SUMMARY.md 4.6, 4.15, 4.22, 4.30 item 3; performance-hotpath.md #6,
  #15, #22, Nits #3; SUMMARY.md Fix Plan P2 #13 (part), #14 (part).

### 20. Remove redundant payload clones and decodes on the commit critical path

- **Priority:** P2
- **Description:** `apply_data_phase` (`tx/commit_phases.rs:145-154`) passes `ops.clone()`
  into the `retry_materialize` closure, so every clean commit deep-copies the per-table
  `Vec<KvOp>` (full record bodies) for a retry that almost never happens. Staged vector
  payloads are cloned once in the delta phase (`:237-242`) and again in `promote_vectors`
  (`:453-458`). The A8 pre-commit scan (`tx/pre_commit.rs:366-377`) msgpack-decodes every
  staged value a second time to collect referenced interner ids, so large batches pay 2-3
  full decode passes per commit.
- **Work:** take `&[KvOp]` in `apply_data_batch` (the MVCC arm only reads it) or clone lazily
  from attempt 2; borrow from `tx.staged_vectors` across those awaits; capture referenced
  interner ids at stage time so the A8 scan amortizes to ~zero.
- **Source findings:** SUMMARY.md 4.7, 4.25, 4.26; performance-hotpath.md #7, #25, #26;
  SUMMARY.md Fix Plan P2 #13 (part), #14 (part).

### 21. Eliminate per-row allocations in filter evaluation and projection

- **Priority:** P2
- **Description:** On the engine's hottest CPU path: the `InSet` probe allocates a fresh
  `String`/`Vec<u8>` per string/binary row value (`query/filter/filter_node.rs:98-99`),
  defeating the borrow-based `scalar_at` design; `$contains_all` deep-clones the required-values
  set per record (`:808`); `$contains` always `materialize_at`s an owned copy plus a
  `QueryValue` conversion instead of the borrowed `str_at` its `Like`/`Regex` siblings use
  (`:670-684` vs `:663-668`); the raw msgpack pre-filter builds an owned `FilterValue` per
  Compare node per record (`eval_bytes.rs:527-539`, helper `:655-667`) despite its
  "zero-alloc raw cursor" contract (`:642`); a `FieldRef` cache hit clones the path `SmallVec`
  per row (`resolve.rs:297-306`); and `SelectProjection` clones the output-key `String` per
  field per record (`query/read/select_projection.rs:231-241`, a documented tradeoff).
- **Work:** borrow-friendly probe key for the `$in` set (or a bytes-keyed mirror set compiled
  once); per-node scratch of positions or a bitmask for `$contains_all`; `str_at` fast path for
  `Contains`; `(RawScalar, &QueryValue)` compare arms in `eval_bytes`; pass the cached path
  slice by reference; `Rc<str>`/`Arc<str>` output keys.
- **Source findings:** SUMMARY.md 4.8, 4.9, 4.10, 4.19, 4.30 items 1 and 2;
  performance-hotpath.md #8, #9, #10, #19, Nits #1, #2; SUMMARY.md Fix Plan P2 #13 (part),
  #14 (part).

### 22. Plan and validate a `ForEach` body once, not per iteration

- **Priority:** P2
- **Description:** Each of up to `ITERATION_CAP` iterations (`query/batch/query_runner.rs:846-944`)
  recurses into `run_nested_body_in_outer_tx` / `execute_batch_impl`, which re-run
  `BatchPlanner::plan` + `validate_tables` (async table resolution) + `validate_filter_depth`
  (`:210-212`) for a body that is byte-identical across iterations; only the bound params
  differ. A `for_each` over 10k elements with a 5-query body in a 200-table repo pays 10k ×
  (plan + 5 resolves + validation) of redundant work.
- **Work:** plan/validate once before the loop; per iteration run only the deadline check,
  param injection, and execution.
- **Source findings:** SUMMARY.md 4.12; performance-hotpath.md #12; SUMMARY.md Fix Plan
  P2 #13 (part).

### 23. Vectorize per-item awaited store round-trips: drainer index postings and `HISTORY OF`

- **Priority:** P2
- **Description:** Drainer Phase A routes every `IndexPut`/`IndexDel` through `replay_v2_op`
  (`tx/drainer.rs:419-437` → `tx/recovery.rs:147-239`) — one awaited `info_store().set/remove`
  plus a `table_by_token` resolve per posting — while data ops are deliberately batched per
  table in Phase B and `Store::transact(Vec<KvOp>)` exists; an index-heavy drain window lags,
  engaging `MAX_UNDRAINED_VERSIONS` backpressure sooner. `read_history`
  (`table/read_temporal.rs:481-488`) issues one awaited `history_of` per matched record where
  the sibling `read_as_of` uses the vectored `get_at_many`.
- **Work:** accumulate index postings per `table_id` in Phase A and transact them per table in
  Phase B (preserving per-op error semantics per batch; sequence after group 1, which
  restructures the same Phase A/B code); add a `history_of_many` mirroring `get_at_many`,
  chunked at the page size.
- **Source findings:** SUMMARY.md 4.11, 4.21; performance-hotpath.md #11, #21; SUMMARY.md
  Fix Plan P2 #13 (part), #14 (part).

### 24. Migration shadow-log: range-scan from `start_lsn`, batched drain, purge on commit, pass budget, key-id validation

- **Priority:** P2
- **Description:** `MigrationShadowLog::read_from` (`migration/shadow_log.rs:105-123`) scans
  the whole `__shadow_<id>_` prefix from LSN 0 on every drain, buffers every entry ≥
  `start_lsn` without cap, then sorts already-ordered keys. Both drain loops
  (`migration/coordinator.rs:212-229`, duplicated at `:288-303`) apply entries via per-entry
  `set`/`remove` round-trips with a `value.clone()` each, although the same file's snapshot
  path uses `set_many`. `final_drain_and_commit` ends at `Committed` without purging
  (`:274-310`; only `rollback` purges at `:319`), so every committed migration leaves its full
  shadow log on disk forever. `drain_until_caught_up` (`:247-260`) loops while `applied > 0`,
  a livelock under sustained source writes. `ShadowKey`/`MigrationShadowLog` never validate the
  documented "no interior `_`" id constraint (`migration/shadow_key.rs:6-8`, `:53-59`,
  `shadow_log.rs:38-46`) even though production ids embed a user-controlled table name.
- **Work:** range-scan starting at `ShadowKey::new(id, start_lsn)` with a page cap and drop
  the sort; group Puts into `set_many` and Deletes into `remove_many`; purge after the phase
  flips to `Committed`; add a pass/attempt budget returning residual lag; validate the id
  charset or length-prefix the id in the key layout.
- **Source findings:** SUMMARY.md 4.13, 4.14, 4.16, 2.5, 5.7; performance-hotpath.md #13,
  #14, #16; concurrency-lockfree.md #5; api-wire-protocol.md #7; SUMMARY.md Fix Plan P2 #13
  (part), #14 (part), #16 (part), #17 (part).

### 25. Schema-validator per-record costs: FK parent semi-join, `one_of` set, path/interner reuse, native adapter round-trip

- **Priority:** P2
- **Description:** `ValidatorDb::exists_in_table` streams the whole parent table per child
  row when the referenced field has no single-field index (`validator/validator_db.rs:261-283`)
  — O(M·P) for M children; the `one_of` rule does a linear `allowed.contains(&actual)` over
  a user-supplied, unbounded list fed by an allocating `materialize_as_qv` per record
  (`validator/schema/field_rule.rs:264-280`); `rule.path.iter()..collect::<Vec<&str>>()` runs
  per rule per record (`schema_validator.rs:130`, `cross_field.rs:80`) and
  `ViewFields::resolve_path` re-collects a fresh `Vec<InternerKey>` with repeated interner
  lookups on every probe (`record_fields.rs:89-114`); the legacy `native_adapter.rs:56` pays a
  full encode→decode `QueryValue` round-trip per invocation even on the accept path.
- **Work:** batch the statement's FK values and semi-join against one parent pass (or
  warn/refuse FK rules on unindexed fields); precompute a `TFxSet` of allowed values at
  validator build time and probe with a borrowed scalar; `SmallVec` path refs and resolve the
  interner path once per (validate-call, field); skip the round-trip on the empty-error path.
- **Source findings:** SUMMARY.md 4.17, 4.28, 4.29, 4.30 item 7; performance-hotpath.md #17,
  #28, #29, Nits #7; SUMMARY.md Fix Plan P2 #14 (part).

### 26. Maintenance-path growth and copy nits: streaming doctor rebuild, DDL op-log FIFO sweep, `stores_list` set

- **Priority:** P2
- **Description:** `doctor::repair` clones the entire materialized table per regular/unique
  index definition (`table/doctor.rs:600-613`, `all_records.clone()`), holding ~D+1 full-table
  copies in RAM although the streaming `create_index_from_stream` exists;
  `maybe_evict_terminal_records` (`table/ddl_op_log.rs:106-120`) is a permanent `Ok(())` stub
  so terminal DDL status records accumulate forever despite the documented cap;
  `stores_list_routed` (`repo/repo_types.rs:205-213`) does `names.contains(&disk_name)` per
  store, O(names × stores), schema-sized.
- **Work:** use the streaming rebuild for the regular index family (keep collect for unique
  until F-78); implement the documented FIFO sweep (newest `DDL_OP_LOG_CAP` terminal records)
  at open and post-terminal-write; flatten the contains loop with a `TFxSet`.
- **Source findings:** SUMMARY.md 4.23, 4.24, 4.30 item 5; performance-hotpath.md #23, #24,
  Nits #5; SUMMARY.md Fix Plan P2 #14 (part).

### 27. Security architecture: type-level authorization seam and replication trust precondition

- **Priority:** P2
- **Description:** Every public engine entry point (`execute_batch`, `batch_execute.rs:79-100`;
  `execute_in_open_tx`; the actor-less `DbInstance` facade) is a full-power API; DAC is enforced
  only if the embedding calls `ShamirDb::execute_as` first, and `trace_access`
  (`query_runner.rs:563-578`) is documented as observability, not the gate — nothing structural
  stops a new server route, WASM host bridge, or internal job from bypassing the wrapper (note
  also that `FilterContext::new` defaults `actor: Actor::System`, `eval_context.rs:87`).
  Replication apply (`tx/apply_replicated.rs:124-271`, trust model at `:4-9`) writes leader
  event bytes with no validator/schema/DAC/integrity check and re-emits them downstream, so
  the whole path's security rests on transport authentication in sibling crates.
- **Work:** consider an `Authorized<BatchRequest>` token minted by the enforcement layer (or
  an enforcing `trace_access` sibling behind a feature flag); document the transport
  precondition in REPLICATION.md's threat model and consider an opt-in validate-on-apply mode.
- **Source findings:** SUMMARY.md 3.3, 3.4; security-crypto.md #3, #4; SUMMARY.md Fix Plan
  P2 #15 (part).

### 28. Concurrency nits: watchdog logging outside scc bucket locks, bounded `FkReverseCache` CAS retry

- **Priority:** P2
- **Description:** The 1 Hz op watchdog thread calls `log::warn!` inside the `iter_sync`
  closure (`query/batch/op_watchdog.rs:118-130`), i.e. synchronous log I/O while scc holds the
  bucket lock that `register_op_watchdog`/`OpGuard::drop` contend on the batch-op path.
  `FkReverseCache::get_or_build_by_parent` retries a lost publish-CAS indefinitely while holding
  `build_lock` (`repo/fk_reverse_cache.rs:342-355`), so a continuous `invalidate()` storm
  (continuous DDL) starves the rebuilder and every waiter — practically bounded, flagged for
  completeness.
- **Work:** collect `(id, alias, elapsed)` triples inside the closure and log after
  `iter_sync` returns (the second pass over `ids_to_update` already shows the shape); add a
  `yield_now` between retries or a bounded retry + error return without releasing `build_lock`
  mid-retry.
- **Source findings:** SUMMARY.md 2.4, 2.6; concurrency-lockfree.md #4, #6; SUMMARY.md Fix
  Plan P2 #16 (part).

### 29. Error-handling polish: log silent best-effort sites, strict validator `stop`, safe `unix_millis`, poison-tolerant guard lock, `thiserror`

- **Priority:** P2
- **Description:** Several best-effort sites drop errors with no log line, unlike every
  sibling best-effort site in the crate: `let _ = save_index2_metadata(..)`
  (`table/table_manager.rs:703`, `table/doctor.rs:716`), `if let Ok(tbl) = repo.get_table(..)`
  in broadcast `IndexPut`/`IndexDel` replay (`tx/recovery.rs:180`, `:231`), and
  `tx_gate()` failure → `ok = false` with no message (`tx/commit_phases.rs:353-355`). The
  validator-result decoder errors on a non-string `code` but silently maps a non-bool `stop`
  to `false` (`validator/decode.rs:59-63`), losing a WASM guest's intent to halt the chain.
  `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()` is repeated 10× in
  `table/table_manager_index_mgmt.rs` (`:1135`, `:1215`, `:1263`, `:1367`, `:1570`, `:1765`,
  `:1858`, `:2274`, `:2435`, `:2636`) where `migration/coordinator.rs:60-62` already uses the
  safe `.unwrap_or(0)` form. `ids.lock().unwrap()` in the sanctioned DDL guard
  (`table/in_flight_create_guard.rs:100`, `:113`, `:143`) would cascade a poisoned lock into
  `degraded_index_count()` panics. `QueryParseError` (`query/common/parser.rs:17-33`) and
  `WriteValueError` (`query/batch/param_subst.rs:145-146`) hand-roll `Display`/`Error`
  instead of `thiserror`.
- **Work:** `log::warn!`/`log::error!` at each silent site stating the accepted consequence;
  add a `BadStopType` variant mirroring `NonStringCode`; extract one safe `unix_millis()`
  helper; `.lock().unwrap_or_else(|p| p.into_inner())` or a comment naming the invariant;
  mechanical `thiserror` conversion when the files are next touched.
- **Source findings:** SUMMARY.md 6.7, 5.6, 6.10, 6.11, 6.12; error-handling-lifecycle.md
  #7, #10 (all three nits); api-wire-protocol.md #6; SUMMARY.md Fix Plan P2 #17 (part), #18.

### 30. Style sweep: hoist mid-function imports, relocate inline test modules, normalize test manifests and naming, fold tail re-exports

- **Priority:** P2
- **Description:** CLAUDE.md structural rules are violated at scale in otherwise-conformant
  code. ~25+ mid-function `use` statements in ~15 production files, none fitting a documented
  exception (e.g. the same `use futures::StreamExt;` three times in
  `migration/shadow_log.rs:50,109,129`; `table/table_manager.rs:16,935,975,1582`;
  `query/read/parser.rs:178` nested inside a match arm; `validator/schema/cross_field.rs:37`
  `use CompareOp::*`; full list in style-claude-md.md #2, plus the additional sites in
  spot-check note 8). Two implementation files embed `#[cfg(test)] mod tests`
  (`query/read/hashable_query_value.rs:250-379`, `table/writer_drain_barrier.rs:410-534`; keep
  the `#[cfg(loom)]` model at `:535`). Test manifests mix `mod`/`pub mod` and add redundant
  per-line `#[cfg(test)]` (`repo/tests/mod.rs`, `repo/group_commit/tests/mod.rs`,
  `query/read/tests/mod.rs:3-8`, `query/batch/tests/executor_tests/mod.rs`); five test files
  lack the `_tests` suffix (`tx/tests/p1096_tx_aware_unique_check.rs`,
  `p1097_remove_posting_owner.rs`, `p1100_stale_snapshot_delete_posting.rs`,
  `p1101_released_skip_durable_check.rs`, `table/tests/f53b_step3_cursor_after_spike.rs`);
  `query/batch/tests/watchdog_tests.rs:159-163` nests a redundant `mod tests`;
  `query/batch/query_runner.rs:1857-1863` carries tail `pub use` re-exports duplicating
  `query/batch/mod.rs`; `repo/repo_types.rs` holds 11 public types (defensible as one
  family; optional composites/factories split).
- **Work:** one or two pure `style:` commits (SHA appended to `.git-blame-ignore-revs`):
  hoist imports; move the two inline test modules to `query/read/tests/hashable_query_value_tests.rs`
  and `table/tests/writer_drain_barrier_tests.rs`; normalize manifests to `pub mod x_tests;`;
  `git mv` the five files; flatten the watchdog wrapper; fold the tail re-exports into
  `mod.rs`. (The `group_commit/mod.rs` move is group 10's first step.) If group 5 deletes the
  legacy parser, `parser.rs:178` disappears with it.
- **Source findings:** SUMMARY.md 7.2, 7.3 (= 1.8), 7.4, 7.5, 7.6, 7.7, 7.8;
  style-claude-md.md #2, #3, #4, #5, #6, #7, #8; correctness-tdd.md #8; SUMMARY.md Fix Plan
  P2 #19 (part).

### 31. Correct the stale footprint-ordering doc in `finalize.rs`

- **Priority:** P2
- **Description:** `tx/finalize.rs:21-26` justifies not unifying the AsyncIndex tail by
  claiming its SSI footprint (`record_commit_writes`) runs *after* `version_guard.commit()`;
  the code records the footprint strictly *before* publish (`tx/commit.rs:744` precedes
  `:751`, with its own F-28/S3-C comment explaining the missed-phantom window this order
  closes). Divergence axis 1 of the three listed is false; a future refactor trusting the doc
  could re-order footprint-after-publish on some path and reintroduce the real window.
- **Work:** update the doc to the current order and re-evaluate whether the remaining two
  axes still justify the duplicated tail. Doc-only, but on a concurrency-critical ordering it
  must not ride inside the style sweep (CLAUDE.md: sweeps are style-only).
- **Source findings:** SUMMARY.md 1.6; correctness-tdd.md #6.

---

## Spot-check notes

Everything load-bearing (all P0/P1 fix-plan items and every HIGH-severity finding) was
re-verified against the current source and **reproduces as described at the cited lines** —
no finding was found to be stale or already fixed (the crate has had no commits since
2026-08-13; #1111's "residual O(N²) clone" fix touched `released_unique_keys_in_tx`, not the
`rederive_stale_value_ops_post_stage` loops in group 6). The following citation/bookkeeping
inaccuracies were found; none changes a finding's substance or severity:

1. **1.2 reachability is narrower than "unattached/system tables" suggests.** The only
   production call to `TableManager::create` is inside `create_table_context`
   (`repo/repo_instance.rs:412`), which unconditionally attaches an `MvccStore` (`:396-399`,
   `:418`); other `TableManager::create` callers are benches, and `install_table_for_test`
   (`:374`) is test-only. So in production a table without an entry in `per_table_mvcc` while
   its WAL entries are still in the drain window arises only via the `remove_table` race
   (`:510` evicts the entry). The defect (false comment, Phase A skipping all data ops, Phase B
   doing nothing, Phase C finalizing anyway, cold/warm divergence) is real and P0 stands; the
   trigger is "DROP TABLE racing an undrained commit", not a standing class of system tables.
2. **SUMMARY.md 1.2 cites `tx_out.rs`** — the file is `crates/shamir-engine/src/tx/tx_outcome.rs`
   (correctness-tdd.md #2 has it right).
3. **SUMMARY.md / api-wire-protocol.md 5.1 cites a re-export at `query/mod.rs:13`** — that line
   re-exports `QueryParseError`, not `query_from_value`. The function is public via
   `query/mod.rs:6` (`pub mod read;`) → `query/read/mod.rs:19` (`pub use parser::query_from_value`),
   so the "public-API trap" substance holds unchanged. No non-test caller exists in the
   workspace (grep over `crates/` excluding `tests/`).
4. **security-crypto.md #3 says the only `Actor::System` hardcode in non-test engine code is
   inside `#[cfg(test)] execute_batch_with_permissions`** — `FilterContext::new`
   (`query/filter/eval_context.rs:87`) also defaults `actor: Actor::System` in production
   code. This strengthens rather than weakens 3.3 (a `FilterContext` built without
   `.with_actor(..)` evaluates `$fn` as System); recorded in group 27.
5. **SUMMARY.md 7.8 / style-claude-md.md #8 cite `crates/shamir-engine/src/repo_types.rs`** —
   the file is `crates/shamir-engine/src/repo/repo_types.rs` (performance-hotpath.md Nits #5 and
   concurrency-lockfree.md cite it correctly). The 11-public-type count is confirmed.
6. **SUMMARY.md 4.30 header says "item 4 merged into 3.7"** — item 4 (the `fk_reverse_cache`
   `Vec<ReverseFkEntry>` clone) is a standalone nit; it is item 8 (the `session.rs` dead loop)
   that merges into 3.7. Correspondingly, Fix Plan #14's "the seven standalone perf nits
   (4.30 items 1-3, 5-7)" lists six ids for seven nits; the standalone set is items 1-7. This
   document assigns all seven (items 1, 2 → group 21; 3 → group 19; 4 → group 9; 5 → group 26;
   6 → group 11; 7 → group 25).
7. **concurrency-lockfree.md #2's caller citation (`validator_management.rs:504`) is
   correct**, but `add_binding` has two more callers (`core.rs:440` boot-time restore,
   single-threaded; `schema_management.rs:598` schema-validator bind). Neither serializes
   against `bind_validator_as`; the race analysis is unchanged.
8. **style-claude-md.md #2's "~25 sites" is, if anything, an undercount.** A `^\s+use` grep
   over non-test `src/` also finds `query/batch/fk_actions.rs:1284`,
   `query/batch/fk_on_update.rs:1092`, `table/table_manager_index_mgmt.rs:2710` (a fourth
   `use futures::StreamExt;` inside a fn), `:2801-2802`, `:3044`, and
   `query/filter/eval_context.rs:71` (`use std::sync::OnceLock;` inside a helper fn), beyond
   the lens' list; `batch_execute.rs:299` and `commit_phases.rs:566` sit inside
   `#[cfg(test)]`-gated bodies and fall under the documented cfg exception.
9. **Minor line-range drift (no substance change):** SUMMARY.md 7.6 cites
   `watchdog_tests.rs:146-163`; the redundant nested `mod tests` starts at `:159` (helpers at
   `:146-157`). SUMMARY.md 7.7 cites `query_runner.rs:1856-1861`; the tail re-export block is
   `:1857-1863`. api-wire-protocol.md #5 says "~30" `filter_from_value` sites in
   `filter_stream_tests.rs`; the count is 27.
10. **Coverage-gap claims verified:** `tx/tests/drainer_tests.rs` has zero references to
    `per_table_mvcc` / `install_table_for_test` / `MvccStore` and 47 `get_table`/`drain_step`
    hits (group 1's "no test covers the unattached path" holds);
    `repo/group_commit/tests/` contains no panicking-flush test (group 10);
    the `TEST_READ_ONE_TX_BYTES_FAILURE` seam exists in `table_manager_streaming.rs` /
    `pre_commit.rs` for reuse by group 2.
