# shamir-engine -- Performance & O(x->0)

## Summary

Reviewed every non-test `.rs` under `crates/shamir-engine/src/` (plus `Cargo.toml`) against CLAUDE.md pillar 3 (O(x→0): batched/amortized over per-row, no hidden O(N)/O(N²), no allocation in loops, no unbounded growth). The crate is largely disciplined — zero un-annotated `scc::*::len()` calls in non-test code, THasher/Fx used everywhere (no `RandomState`), and several paths carry textbook hoisting (e.g. `table_manager_crud.rs:285-291`) — but five hot-path quadratic/unbounded shapes remain: an O(N²·K) re-planning loop in pre-commit under normal concurrency, an O(M²) staged-overlay probe in the validator write path, per-value full child-table scans in FK RESTRICT, a changelog reader that buffers the entire journal tail to return `limit` rows, and a repo-wide schema scan paid by every UPDATE. All HIGH/MEDIUM findings below were re-verified line-by-line by opening the cited files; LOW/nit entries were pattern-verified against the same files.

## Findings

### 1. O(N²·K) re-planning scans inside `rederive_stale_value_ops_post_stage`
- **File:** `src/tx/pre_commit.rs:1999-2329` (rebuild at `:2014-2030`; linear rescans at `:2089-2101` and `:2209-2300`; gates at `:1875`, `:1969-1975`)
- **Severity:** high
- **Issue:** For each staged row, the code (a) rebuilds `staged_removals_by_rid` by re-iterating the whole `tx.index_write_set` filtered to the table and cloning every matching `RemovePosting.key` (`key.clone()` at `:2027`), and (b) for each re-planned op runs a `.iter().filter(|(t,_)| *t == table_token).any(...)` linear rescan of the same set. With N staged rows × K index ops each (E ≈ N·K ops), cost is O(N²·K) map rebuilds, key clones and scans. Unlike its generation-gated siblings, the gate here (`version_allocation_high_water_mark > snapshot_version`, `:1970`) fires under *any* concurrent write traffic — the normal production case for any table with base indexes.
- **Failure scenario:** Bulk DELETE/UPDATE of 10k rows over 3 indexed fields under concurrent commits → ~10^8-element linear scans plus ~30k full map rebuilds with key clones, executed inside the locked pre-commit validate phase, directly widening commit latency for all same-table writers.
- **Suggested fix:** Build `staged_removals_by_rid` (and a `TFxSet` of staged regular posting keys) once per table before the row loop, update it when appending ops; replace both `.any()` rescans with O(1) set lookups.

### 2. Staged-overlay probe clones and linearly scans the whole tx write-set per validated record
- **File:** `src/validator/validator_db.rs:312` (`staged_field_matches`) and `:427-440` (`exists_in_self` step 3); root cost in `crates/shamir-tx/src/staging_store.rs:172-180` (`snapshot_ops()`)
- **Severity:** high
- **Issue:** `staging.snapshot_ops()` materializes a fresh `Vec<KvOp>` cloning every staged key and value bytes, then `.any(...)` linearly scans it — invoked once per unique/FK schema-rule probe, i.e. per record being validated.
- **Failure scenario:** Batch-insert of M rows into a table with a schema-level `unique` or FK rule in one tx → M probes × O(M) clone+scan = O(M²) time and O(M) transient allocations per record, on the hot pre-commit write path (the autocommit path threads an implicit tx, so `ctx.db()` is `Some`).
- **Suggested fix:** Add a non-materializing `for_each_op` iterator to `StagingStore` that matches on borrowed bytes (no key clone), and/or maintain a per-table staged-value set keyed by (interned field id, scalar) updated at stage time for O(1) probes.

### 3. FK RESTRICT: one full child-table scan per parent value, values not deduplicated
- **File:** `src/query/batch/fk_restrict.rs:145-164` (per-value loop); `collect_parent_values` `:220-282` (un-deduped push at `:273`); full-scan fallback in `child_has_reference` `:373-391`
- **Severity:** high
- **Issue:** `for parent_val in values_for_field { child_has_reference(...) }` — when the child FK column has no single-field index, each value pays a full `list_stream_tx` scan of the child table. `collect_parent_values`' doc says "distinct values" but `:273` pushes every matched row's value, so duplicates multiply identical scans. The index fast path (`:325-352`) exists but only when an index covers the field.
- **Failure scenario:** Bulk delete of 10k parent rows whose FK value repeats (deleting one customer's orders) against a 1M-row child table without a child-side index → up to 10k full child scans inside one delete op — minutes of latency.
- **Suggested fix:** Dedupe parent values into a `TFxSet`, then invert: ONE pass over the child table testing each row's FK field against a coercing membership set (the shape `fk_actions::classify_row` already uses), keeping the index fast path as an early-out per distinct value.

### 4. Changelog `range_from` buffers the entire journal tail, then truncates to `limit`
- **File:** `src/repo/changelog_store.rs:37-55`
- **Severity:** high
- **Issue:** `iter_range_stream(Some(from_key), None, batch)` has no upper bound; the loop drains ALL events ≥ `from_key` into a `Vec`, sorts the whole thing (O(M log M)), then `truncate(limit)`. Cost/memory is O(journal tail) where O(limit) is the contract. The changelog journal has no retention (server repl handler documents "R0: no retention"), so M grows forever.
- **Failure scenario:** A follower doing changefeed catch-up polls `read_changelog_from` → `range_from`; on a cold start (or any long-lived journal) each poll reads, buffers, and sorts the entire history of every commit ever made — unbounded RAM per poll and O(tail) I/O per tick.
- **Suggested fix:** Early-exit the collect loop once `limit` records are gathered (keys are big-endian commit versions, so chunks arrive ascending on disk backends); keep a defensive sort of only those `limit` items.

### 5. ON UPDATE discovery: repo-wide table scan on EVERY UPDATE, before the no-op gate
- **File:** `src/query/batch/fk_on_update.rs:734-783` (`discover_on_update_refs`); invoked at `:188`, before the set-fields ∩ ref-fields gate at `:196-204`
- **Severity:** high
- **Issue:** `repo.list_table_names()` then per table `resolver.resolve(...)` + `child_table.collect_fk_refs()` — an O(tables) schema walk per UPDATE op, paid even when the update touches no FK-referenced field (the intersection gate runs after it). This is exactly the scan F-28 Step 4 (#831) removed from the delete path via `RepoInstance::fk_reverse_cache`; the ON UPDATE path never migrated, contradicting the module's own "zero scan overhead on the hot path" claim.
- **Failure scenario:** Repo with 500 tables under update-heavy load → 500 table resolves + FK collections per UPDATE statement, dominating op latency.
- **Suggested fix:** Route through `repo.fk_reverse_cache()` entries filtered on `on_update != NoAction` (mirror `discover_action_refs` in `fk_actions.rs`), then apply the cheap intersection gate first.

### 6. Per-row `iter_unique_indexes()` clone storm in tx batch insert
- **File:** `src/table/table_manager_tx_ops.rs:705-722` (`insert_tx_many`), `:920-938` (`insert_tx_many_bytes`); clone source `crates/shamir-index/src/base_index/index_info.rs:310-314`
- **Severity:** medium
- **Issue:** `for (i, v) in values.iter().enumerate() { for def in self.index_manager.iter_unique_indexes() {...} }` — the iterator yields owned `IndexDefinition` clones (`snap[i].clone()`), so a batch pays O(rows × U × deep-clone(IndexDefinition)). The sibling non-tx path fixed precisely this with an explicit flamegraph-justified hoist (`table_manager_crud.rs:285-291`); the two tx paths (the wire path for transactional INSERT) did not.
- **Failure scenario:** 10k-row transactional bulk insert into a table with 4 unique indexes → 40k deep `IndexDefinition` clones + iterator setups instead of 4.
- **Suggested fix:** Hoist `let unique_defs: Vec<IndexDefinition> = self.index_manager.iter_unique_indexes().collect();` above the row loop in both methods, mirroring `crud.rs:290`.

### 7. Phase 5a data batch deep-cloned on every commit for a retry that almost never happens
- **File:** `src/tx/commit_phases.rs:145-154` (`apply_data_phase`); `src/tx/materialize.rs:80-89` (`materialize`)
- **Severity:** medium
- **Issue:** `retry_materialize(MATERIALIZE_ATTEMPTS, || { apply_data_batch(repo, table_id, base.clone(), ops.clone(), ...) })` — the `FnMut` closure body runs on attempt 1 too, so every clean commit allocates and memcpys the entire per-table `Vec<KvOp>` (full record bodies) once, purely so a rare retry could re-run it.
- **Failure scenario:** Every commit pays an extra O(tx payload) alloc+memcpy on the latency-critical ack path; large batch commits double the write path's memory bandwidth.
- **Suggested fix:** Take `&[KvOp]` in `apply_data_batch` (the MVCC arm already only reads it), or hold `Option<Vec<KvOp>>` and clone lazily from attempt 2 onward.

### 8. `$in` probe heap-allocates per string/binary row value
- **File:** `src/query/filter/filter_node.rs:98-99`
- **Severity:** medium
- **Issue:** `ScalarRef::Str(s) => set.contains(&QueryValue::Str(s.to_string()))` (and `Bin(b.to_vec())`) — a fresh heap allocation per field probe per row on the `InSet` arm, the engine's hottest CPU filter path (see the `filter_eval` bench). The F6 borrow-based `scalar_at` design (documented "zero clone" at `:547`) is defeated for the two variable-length scalar types.
- **Failure scenario:** `$in` filter over a 1M-row string column with a 100-element literal set → 1M unnecessary String allocations per query.
- **Suggested fix:** Give the set a borrow-friendly probe: a wrapper key type that `Hash`es/`Eq`s over `&str`/`&[u8]` bytes against the `TSet<QueryValue>`, or compile a bytes-keyed mirror set at filter-compile time.

### 9. `$contains_all` deep-clones the required-values set per record
- **File:** `src/query/filter/filter_node.rs:808`
- **Severity:** medium
- **Issue:** `let mut remaining = values.clone();` per record — deep-copies every `QueryValue::Str` in the client-supplied `TSet` for every row evaluated. The in-comment acknowledgment ("The only allocation is the cloned scratch set") is per-row, not per-query.
- **Failure scenario:** `$contains_all` with a 50-element string list over 1M rows → 1M × 50-string deep clones — O(rows × M) time and allocation churn.
- **Suggested fix:** Reuse a per-node scratch of *positions* (`Vec<u32>` cleared in place between rows) or a ≤64-bit bitmask like the neighboring `FtsMatch` code (`:897`).

### 10. Raw msgpack pre-filter allocates per Compare node per record despite the "zero-alloc" contract
- **File:** `src/query/filter/eval_bytes.rs:527-539`, conversion helper `:655-667` (contract comment at `:642`)
- **Severity:** medium
- **Issue:** For every record × every `Compare` node, `query_value_to_filter_value_lit(pre)` builds an owned `FilterValue` — `FilterValue::String(s.clone())` / `Binary(b.clone())` for Str/Bin literals — or falls back to `value.clone()`. The module header (`:32-36`, `:642`) claims a zero-alloc raw cursor; string/binary comparisons break that.
- **Failure scenario:** A `WHERE str_field = ?` bytes pre-filter over a large scan allocates one String per record per Compare node ahead of the full decode it is supposed to cheaply gate.
- **Suggested fix:** Add `(RawScalar, &QueryValue)` compare arms that compare the raw slice directly (`RawScalar::Str(a)` vs `&QueryValue::Str(s)` → `a.cmp(s.as_bytes())`), removing both the conversion and the clone.

### 11. Drainer Phase A applies index postings one awaited store op at a time
- **File:** `src/tx/drainer.rs:419-437` → `src/tx/recovery.rs:147-239`
- **Severity:** medium
- **Issue:** Data ops are deliberately accumulated and applied per-table in Phase B, but every `IndexPut`/`IndexDel` routes through `replay_v2_op` → one awaited `info_store().set/remove` per posting (plus a `table_by_token` resolve per op), although `Store::transact(Vec<KvOp>)` batching exists and the same file's own Phase B rationale is to coalesce exactly this shape.
- **Failure scenario:** An index-heavy drain window (bulk ingest, backfill) does one await + one backend op per posting instead of one per table; drain throughput lags, `MAX_UNDRAINED_VERSIONS` backpressure engages sooner and brakes live commits.
- **Suggested fix:** Accumulate `IndexPut`/`IndexDel` per `table_id` in Phase A and transact them per table in Phase B (the per-op error semantics can be preserved per batch).

### 12. ForEach re-plans and re-validates an identical body every iteration
- **File:** `src/query/batch/query_runner.rs:846-944`; the duplicated setup at `:210-212` (`BatchPlanner::plan` + `validate_tables` + `validate_filter_depth`)
- **Severity:** medium
- **Issue:** Each of up to 100k iterations (`ITERATION_CAP` at `:35`) recurses into `run_nested_body_in_outer_tx` / `execute_batch_impl`, which re-runs planning + async table resolution + filter-depth validation for a body that is byte-identical across iterations; only params differ.
- **Failure scenario:** `for_each` over a 10k-element list with a 5-query body in a 200-table repo pays 10k × (plan + 5 table resolves + validation) of redundant work before any execution.
- **Suggested fix:** Plan/validate once before the loop; per iteration run only deadline-check, param injection, and execution.

### 13. Shadow log `read_from` rescans the whole prefix and buffers unbounded
- **File:** `src/migration/shadow_log.rs:105-123`
- **Severity:** medium (admin/migration path)
- **Issue:** Every drain scans the full `__shadow_<id>_` prefix from lsn 0 (skipping deserialize, not reads, of old entries), buffers every entry ≥ `start_lsn` without cap, then `sort_by_key` defensively — although keys are big-endian-LSN-suffixed and thus already lexicographically ordered.
- **Failure scenario:** Long migration with L entries drained D times (`drain_until_caught_up` loops at `coordinator.rs:247-260`; admin drains at start and cutover) → O(D·L) storage reads and O(tail) RAM per drain.
- **Suggested fix:** Range-scan starting at `ShadowKey::new(id, start_lsn)`, drop the sort (or assert ordering), and cap the returned page like `MAINT_SCAN_BATCH` does elsewhere.

### 14. Shadow drain applies entries via per-entry `set`/`remove` round-trips
- **File:** `src/migration/coordinator.rs:212-229` and duplicated at `:288-303` (batched contrast in the same file at `:166-185`)
- **Severity:** medium (admin/migration path)
- **Issue:** `self.dst_data.set(key, Bytes::from(value.clone())).await?` per entry, while the same file's snapshot path correctly uses `set_many`. N entries → N individual async store ops (each potentially its own WAL/fsync) + a `value.clone()` per entry.
- **Failure scenario:** Final drain before cutover after hours of dual-writes can hold hundreds of thousands of entries; per-op overhead makes cutover latency grow linearly where a batched sweep would be an order of magnitude faster.
- **Suggested fix:** Group Puts into `set_many` chunks and Deletes into `remove_many` (API already used by `shadow_log.purge:137`).

### 15. `index2_on_insert` re-snapshots all index2 backends per record on the batched non-tx insert path
- **File:** `src/table/table_manager_crud.rs:373-375` calling `:89-97`; contrast the tx path's hoist at `table_manager_tx_ops.rs:771-777`
- **Severity:** medium
- **Issue:** `for (id, value) in pairs_iter() { self.index2_on_insert(id, value).await?; }` — each call walks `index2_registry.all_backends()` (an async scc traversal + `Arc` clone + fresh `Vec`, whose own `// O(N) ack` says "off hot path") against a backend set that cannot change mid-batch. The file's own doc (`:236`) acknowledges "index updates still loop per-record"; the two tx batch paths already solved this.
- **Failure scenario:** 1000-row bulk insert into a table with fts/functional/vector backends → 1000 scc-map walks + 1000 Vec allocations to plan against the same unchanged backends.
- **Suggested fix:** Take `let backends = self.index2_registry.all_backends().await;` once before the pair loop and inline the plan/apply body (same shape as `insert_tx_many` step 4).

### 16. Shadow log never purged after a successful migration commit
- **File:** `src/migration/coordinator.rs:274-310` (ends at `Committed` with no purge; only `rollback` purges at `:319` — grep confirms no other caller)
- **Severity:** low (unbounded growth, low rate)
- **Issue:** Every committed migration leaves all `__shadow_<id>_<lsn>` records (full row values) in the store forever; repeated migrations accumulate monotonically.
- **Failure scenario:** Disk grows without bound across migrations; future prefix scans (including finding #13's rescans) get slower with each migration.
- **Suggested fix:** Call `shadow_log.purge()` after the phase flips to `Committed` (or in the admin cleanup that removes the coordinator).

### 17. FK `exists_in_table` fallback: full parent-table scan per child record
- **File:** `src/validator/validator_db.rs:261-283`
- **Severity:** low
- **Issue:** When the referenced parent field has no single-field index, each child-row FK validation streams the whole parent table (`list_stream` + per-row `record_field_matches` with interner lookups). Documented behavior, but nothing warns or wires an index.
- **Failure scenario:** Inserting M children whose FK targets an unindexed parent column scans O(M·P) parent rows.
- **Suggested fix:** Batch the statement's FK values and semi-join against one parent pass, or warn/refuse FK rules whose field lacks a ready single-field index.

### 18. ON-UPDATE RESTRICT gate repeats the per-changed-value scan shape with un-deduped values
- **File:** `src/query/batch/fk_on_update.rs:337-355` (gate), `:304-316` (un-deduped `restrict_fields` build)
- **Severity:** low
- **Issue:** Same family as finding #3: per-old-value child probes, duplicates included, on the ON UPDATE path.
- **Suggested fix:** Same as #3 — dedupe + single child pass against a membership set.

### 19. `$contains` ignores the borrowed `str_at` fast path its siblings use
- **File:** `src/query/filter/filter_node.rs:670-684` (contrast `Like|Regex` at `:663-668`, `FtsMatch` at `:880-883`)
- **Severity:** low
- **Issue:** `Contains` always `materialize_at` (owned copy) + converts to `QueryValue` per row, though the dominant Str-contains-Str case could be served by the borrowed `record.str_at(field_path)`.
- **Failure scenario:** Substring filter over a 1M-row string column pays a full owned String materialization + conversion per row.
- **Suggested fix:** Try `str_at` first (borrowed `s.contains(sub)` when the filter value is a str literal); fall back to materialize only for containers.

### 20. `classify_row*` re-parses RecordView per probe per row
- **File:** `src/query/batch/fk_actions.rs:1192-1246`; mirrored in `fk_on_update.rs:1017-1079`
- **Severity:** low
- **Issue:** Each field probe constructs its own `RecordView::new(bytes)` (map-header walk), and a failed view falls back to `Bytes::copy_from_slice` + full msgpack decode per (row, probe) pair instead of once per row.
- **Suggested fix:** Hoist one `RecordView` (and one fallback decode) per row and match all probes against it.

### 21. `read_history` issues one awaited `history_of` per matched record
- **File:** `src/table/read_temporal.rs:481-488`
- **Severity:** low
- **Issue:** Sequential per-record MVCC round-trips on a path whose siblings are batched (`read_as_of` uses `get_at_many` at `:218-219`).
- **Failure scenario:** `HISTORY OF` over a WHERE matching 50k rows = 50k serialized store awaits.
- **Suggested fix:** Add a vectored `history_of_many` mirroring `get_at_many`, chunked at the page size.

### 22. `execute_update_tx`: per-row `table_token()` recompute + probe allocation
- **File:** `src/table/write_exec.rs:664-667`; token derivation at `table_manager.rs:15-20`
- **Severity:** low
- **Issue:** Inside the matched-rows loop, `tx.write_set.get(&self.table_token())` re-hashes the immutable table name (SipHash over the full name) per row, plus a 16-byte `id.to_bytes()` allocation just to probe staging.
- **Failure scenario:** UPDATE matching 500k rows → 500k redundant name hashes + staging probes that hoisting to two lines before the loop removes.
- **Suggested fix:** Hoist `let token = self.table_token(); let staging = tx.write_set.get(&token);` above the loop; probe with `id.as_bytes()`.

### 23. `doctor::repair` deep-clones the entire materialized table per index rebuild
- **File:** `src/table/doctor.rs:600-613`
- **Severity:** low (operator-triggered maintenance)
- **Issue:** `all_records.clone()` per regular/unique index definition — O(D × N) deep `InnerValue` clones and transient RAM, while a streaming alternative (`create_index_from_stream`) already exists and is used by `create_index`.
- **Failure scenario:** `repair()` on a 50M-row table with 6 indexes holds ~7 full-table tree copies in RAM simultaneously.
- **Suggested fix:** Use the streaming rebuild for the regular family (keep collect only for unique until F-78 lands).

### 24. DDL op-log cap is documented but never enforced (no-op eviction stub)
- **File:** `src/table/ddl_op_log.rs:106-120` (cap constant at `:26`)
- **Severity:** low
- **Issue:** `maybe_evict_terminal_records` is a permanent `Ok(())` stub; terminal DDL status records accumulate one blob per CREATE/DROP/RENAME forever (ingestion rate is DDL-only, hence low).
- **Suggested fix:** Implement the documented FIFO sweep (keep newest `DDL_OP_LOG_CAP` terminal records), triggered at open time and/or post-terminal-write.

### 25. A8 pre-commit scan re-decodes every staged record value on every commit
- **File:** `src/tx/pre_commit.rs:366-377`
- **Severity:** low
- **Issue:** Whenever the tx staged any writes, each staged value is msgpack-decoded again (`InnerValue::from_bytes`) to collect referenced interner ids — a second full decode pass of the commit payload before Phase 5a writes it. Bounded by the tx's own payload and mandated by A8 fail-safety, but large batches pay 2-3 full decode passes per commit.
- **Suggested fix:** Capture referenced ids at stage time (when bytes are first encoded into staging), amortizing the scan to ~zero.

### 26. Staged vector payloads deep-cloned twice per vector commit
- **File:** `src/tx/commit_phases.rs:237-277` and `promote_vectors` at `:453-508`
- **Severity:** low
- **Issue:** Whole `Vec<(RecordId, Vec<f32>)>` embedding lists are cloned once in the delta phase and again in promote; bounded by the tx's own staged vectors, but for 1536-dim batches the avoidable memcpy is significant.
- **Suggested fix:** Borrow from `tx.staged_vectors.get(&token)` across those awaits (the map is not otherwise mutated there).

### 27. Validator registry helpers do full-map scans, one nested inside a loop
- **File:** `src/validator/registry.rs:128-145` (`remove` reverse-scan), `:195-218` (`unbind_all_for_table` → `name_for_id` full scan per candidate)
- **Severity:** low (catalogue-scale, admin path)
- **Issue:** O(V²) worst case for V validators bound to a table; V is schema-sized so small, but the inverse map would make it O(1).
- **Suggested fix:** Store the name alongside the artifact (or keep a direct id→name map) so `name_for_id` is a lookup.

### 28. `one_of` rule: linear `Vec::contains` with a materializing clone per record
- **File:** `src/validator/schema/field_rule.rs:264-280` (linear `allowed.contains(&actual)` at `:280`, fed by allocating `materialize_as_qv`)
- **Severity:** low
- **Issue:** K-element linear scan (K is user-supplied and unbounded) plus a per-check value allocation, per record × per rule.
- **Suggested fix:** Precompute a `TFxSet` of allowed values once at validator build time; probe with a borrowed scalar.

### 29. Per-record/per-rule small allocations across the schema validation path (grouped)
- **File:** `src/validator/schema/schema_validator.rs:130`; `src/validator/schema/cross_field.rs:80`; `src/validator/record_fields.rs:93`
- **Severity:** low
- **Issue:** `rule.path.iter()...collect::<Vec<&str>>()` per rule per record (SmallVec pattern already exists in `validator_binding.rs:15`); `ViewFields::resolve_path` re-collects a fresh `Vec<InternerKey>` and repeats interner lookups on every scalar/str/present probe, several times per rule per record.
- **Suggested fix:** SmallVec for path refs; resolve the interner path once per (validate-call, field) and reuse.

### Nits
- `src/query/filter/resolve.rs:297-306` — FieldRef cache hit clones the path SmallVec per row (inline ≤4 segments, heap beyond); pass the cached slice by reference since `materialize_at` only needs `&[InternerKey]`.
- `src/query/read/select_projection.rs:231-241` — output-key `String` clone per field per record; documented deliberate tradeoff, `Rc<str>` would remove it.
- `src/table/table_manager.rs:15-20` + `:992-994` — `table_token()` re-derives a SipHash over the immutable table name on every call (2-5× per write op); compute once and store the `u64`.
- `src/repo/fk_reverse_cache.rs:419-428` — warm-cache lookup clones the parent's `Vec<ReverseFkEntry>` (String fields) per hit; `Arc<[ReverseFkEntry]>` values remove it.
- `src/repo/repo_types.rs:205-213` — `!names.contains(&disk_name)` per store is O(names × stores); bounded (schema-sized), a `TFxSet` flattens it.
- `src/validator/registry.rs:237-239` — scc `is_empty()` shares O(N)-flavored cost with the annotated `len()` at `:231` but carries no ack (clippy bans only `len`); by pillar-3 spirit it deserves the same treatment.
- `src/validator/native_adapter.rs:56` — legacy adapter pays a full encode→decode `QueryValue` round-trip (≥3 allocs) per invocation even on the empty-error accept path; modern `NativeRecordValidator` avoids it.
- `src/query/auth/session.rs:164-170` — dead `row_filter` loop iterates the whole `row_filters` vec producing nothing (test scaffolding); wasted O(N) scan.

### Verified clean (theme-relevant)
No un-annotated `scc::*::len()` in non-test code anywhere in the crate; the four existing sites (`tx/commit.rs:303`, `:1054`; `tx/drainer.rs:244` test-only; `validator/registry.rs:231`) all carry sound `// O(N) ack:` justifications, and `Drainer::window_depth` is the canonical AtomicUsize mirror working as documented. THasher/Fx discipline holds throughout (no `HashMap::new`/`RandomState` collections). `cond_cache`/`query_ref_cache`/`field_path_cache` are pointer-keyed, per-query-lifetime structures — no unbounded cross-query growth. The drainer window, group-commit waiter queue, changefeed footprint, and FTS/regex precompilation patterns are all correctly bounded.
