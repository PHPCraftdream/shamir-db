# S5c-X Round 9 — integration-test migration off demoted `TableManager` getters

Sweep + migration of every `crates/*/tests|benches|examples` call site of the getters demoted to `pub(crate)` (`.interner(`, `.info_store(`, `.data_store(`, `.index_manager_ref(`, `.index2_registry(`, `.validator_registry_ref(`, `.sorted_indexes(`, `.unique_write_lock(`, `.mvcc_store(`, `.table(`). Engine crate-internal `src/**/tests` cfg(test) mods are OUT of scope.

## Sweep method

`rg -n "\.interner\(|\.info_store\(|\.data_store\(|\.index_manager_ref\(|\.index2_registry\(|\.validator_registry_ref\(|\.sorted_indexes\(|\.unique_write_lock\(|\.mvcc_store\(|\.counter\(|\.table\(" crates/*/tests crates/*/benches crates/*/examples` from the worktree root.

## Sweep results (complete)

Hits requiring migration — all in `crates/shamir-engine/tests/`:

- `crash_recovery.rs:148` `tbl.interner().get()` (minting `touch_ind` in `field_id`)
- `crash_recovery.rs:215` `tbl.index2_registry().all_backends()`
- `crash_recovery.rs:1067` `tbl.interner().get()` (codec decode in `read_all_records`)
- `crash_recovery.rs:1297` `tbl.index2_registry().all_backends()`
- `crash_recovery.rs:1304` `tbl.info_store().clone()` (prefix-scan delta chunks)
- `overlay_depth_probe.rs:41` `tbl.interner().get()` (minting `touch_ind` in `field_id`)
- `string_agg_caps.rs:112` `table.interner().get()` (build `FilterContext`)

Negative sweep (clean — no demoted-getter hits):
- `crates/shamir-engine/tests/clean_horizon/` — clean
- `crates/shamir-engine/benches/` — clean
- `crates/shamir-db/tests/` + `benches/` — `.table(` hits only at `functions_lifecycle.rs:601,602,615,619` on `ctx.db().table(...)` — the PUBLIC `Database::table` API, not the demoted `TableManager` getter → in scope-negligible, unchanged
- `crates/shamir-sdk/tests/` + `benches/`/`examples/` — `procedure_compile_pass.rs:13` is a comment mentioning `ctx.db().table("users")` (public API) → unchanged
- `crates/shamir-server/`, `shamir-connect/`, `shamir-client/`, `shamir-tx/`, `shamir-storage/`, `shamir-wal/`, `shamir-types/`, `shamir-collections/`, `shamir-index/`, `shamir-query-*`, `shamir-funclib/`, `shamir-wasm-host/`, `shamir-numa/`, `shamir-bench-utils/`, `shamir-transport-*`, `shamir-tunables/` — tests/benches/examples all clean
- `crates/shamir-engine/tests/prealloc_caps.rs:111` — already migrated (reference shape), untouched

## Per-site migration (old → new)

- `crates/shamir-engine/tests/crash_recovery.rs:148` (`field_id`, minting) → `crash_recovery.rs:147-166`: signature `field_id(repo: &RepoInstance, tbl: &TableManager, name)`; body now `let permit = repo.write_domain().admit().await.unwrap();` + `tbl.intern_query_value_admitted(&permit, &QueryValue::Map(probe))` (one-field map `{name: Null}`), id read back from the returned `InnerValue::Map` key. Rationale: `touch_ind` is a dictionary WRITE — the only public path is the admitted WRITE-prep op (identical `touch_ind` semantics underneath). Callers updated: :292, :515, :516, :578, :1262, :1507.
- `crates/shamir-engine/tests/crash_recovery.rs:215` (`fts_hit_count`) → now `:223`: `let backends = tbl.index2_read_view().backends().await;` — assertions unchanged.
- `crates/shamir-engine/tests/crash_recovery.rs:1067` (`read_all_records`) → now `:1077-1080`: `tbl.info_store_read_view().interner_read_view().await.unwrap()` then `interner.inner_to_query_value(&record).expect("to_query_value")` (identical codec semantics via `InternerReadView::inner_to_query_value`).
- `crates/shamir-engine/tests/crash_recovery.rs:1297` (`delta_chunk_count`) → now `:1311`: `let backends = tbl.index2_read_view().backends().await;`
- `crates/shamir-engine/tests/crash_recovery.rs:1304` (`delta_chunk_count` info-store scan) → now `:1319-1321`: `tbl.info_store_read_view().scan_prefix_stream(prefix.into(), 100)` — raw-clone replaced by read-view prefix scan; counting loop unchanged.
- `crates/shamir-engine/tests/overlay_depth_probe.rs:41` (`field_id`) → now `:40-59`: same admitted-interning rewrite as crash_recovery; callers updated :78, :121, :164, :204.
- `crates/shamir-engine/tests/string_agg_caps.rs:112-114` → now `:112-117`: `let ctx = table.info_store_read_view().filter_context(&refs).await.unwrap().with_max_accumulator_bytes(100 * 1024);` (matches the prealloc_caps.rs:111 reference shape; the now-unused `shamir_engine::query::filter::FilterContext` import was removed).

## New public API

All in `crates/shamir-engine/src/table/write_receipt/admission/read_only_views.rs`:
- `Index2ReadView` struct (private `Arc<crate::index2::IndexRegistry>`; mutators unreachable; documented that mutation goes through the engine's admitted DDL apply hooks — `plan_*` are pure `&self`):
  - `async fn backends(&self) -> Vec<Arc<dyn IndexBackend>>` (read-only twin of `IndexRegistry::all_backends`)
  - `async fn all_descriptors(&self) -> Vec<IndexDescriptor>` (read-only twin of `IndexRegistry::all_descriptors`)
- `TableManager::index2_read_view(&self) -> Index2ReadView`
- `TableInfoStoreReadView::scan_prefix_stream(&self, prefix: bytes::Bytes, batch_size: usize) -> Pin<Box<dyn Stream<Item = DbResult<Vec<(Bytes, Bytes)>>> + Send + '_>>` (read-only twin of `Store::scan_prefix_stream`; maps the crate-private `RecordKey` item key to `bytes::Bytes` so the public item type is nameable)

Re-exports extended: `crates/shamir-engine/src/table/table_manager.rs:40-43` and `crates/shamir-engine/src/table/mod.rs:53-56` (`Index2ReadView` added).

## Notes / unverified

- No cargo/clippy/fmt run (per brief); compile-readiness was checked by re-reading every changed file. Points to watch in CI: (a) `scan_prefix_stream`'s boxed-stream return coerces from the crate-private `RecordStream` via `.map` (private_interfaces-safe); (b) `filter_context`'s returned `FilterContext` borrows the manager (`'a`), composing with the later `table.read(&query, &ctx)` in string_agg_caps; (c) `RepoInstance::write_domain().admit()` is public (`repo_instance.rs:442`, `write_domain.rs:138`), so the admitted interning path is reachable from integration tests without `test-util`.
- Assertions are byte-for-byte unchanged; only access paths and the field_id minting route changed (semantics identical: same `touch_ind` interning under admission).

## Round 10 addendum

Task: migrate the last REAL `RepoInstance::add_table` call sites in
benches / integration tests to the public admitted twin
(`RepoInstance::add_table_admitted(config, &permit)`, pre-existing at
`crates/shamir-engine/src/repo/changelog_store/write_domain_membership.rs:13`;
repo admission via `RepoInstance::write_domain()` →
`RepoWriteDomain::admit()`). No new API was needed.

### Hit table (add_table — all migrated)

| file:line (pre-change) | method | receiver | action |
|---|---|---|---|
| crates/shamir-engine/benches/f78_writer_latency.rs:128 | add_table | RepoInstance | migrated (async `make_table`: admit → `add_table_admitted` → drop permit) |
| crates/shamir-engine/benches/p1099_touched_probe.rs:105 | add_table | RepoInstance | migrated (bench setup closure, per-iteration fresh instance) |
| crates/shamir-engine/benches/p1107_stale_value_gate.rs:129 | add_table | RepoInstance | migrated (same shape) |
| crates/shamir-engine/benches/p1107_stale_value_gate.rs:279 | add_table | RepoInstance | migrated (same shape) |
| crates/shamir-engine/benches/p1108_released_unique_growth.rs:70 | add_table | RepoInstance | migrated (same shape) |
| crates/shamir-engine/benches/tx_concurrent.rs:210 (`make_repo`) | add_table | RepoInstance | migrated: `make_repo` made `async`; its 3 setup-closure callers (disjoint_inserts, hot_key_snapshot, hot_key_serializable) now create the repo inside the async block (`make_repo().await`). Timed sections unchanged |
| crates/shamir-engine/benches/tx_concurrent.rs:293 | add_table | RepoInstance | migrated: admit + `add_table_admitted` + `drop(permit)` per loop iteration BEFORE the `get_table` await (cold path re-admits; domain mutex non-reentrant) |
| crates/shamir-engine/benches/tx_pipeline.rs:86 (`make_repo`) | add_table | RepoInstance | migrated: `make_repo` stays sync (all callers are sync `main`); admits via a local current-thread tokio runtime (`block_on`) |
| crates/shamir-engine/benches/tx_pipeline.rs:432 | add_table | RepoInstance (loop-local `repo`) | migrated: admit on THAT repo's domain via the loop's existing `rt`; permit dropped before `get_table` |
| crates/shamir-engine/tests/overlay_depth_probe.rs:29 | add_table | RepoInstance | migrated (async `make_repo`) |

### flush_persist_once_tests.rs (interner demotion follow-through)

`crates/shamir-engine/tests/flush_persist_once_tests.rs` (no test-util):

| file:line (pre-change) | method | receiver | action |
|---|---|---|---|
| :42 | `interner().get()` + `query_value_to_inner` | TableManager (interner `pub(crate)`) | migrated: `table.intern_query_value_admitted(&permit, &request)` — the old call was WRITE-prep interning (mutates the dictionary), which a read view cannot do; the admitted twin is the sanctioned replacement (`admitted_ops.rs`) |
| :51 | `interner().persist()` | TableManager | migrated: `repo.write_domain().admit()` → `table.persist_interner_admitted(&permit)` (pub, `admitted_ops.rs:182`); permit dropped before `flush_buffers` |
| :84 | `interner().get()` + `inner_value_to_query_value` | TableManager | migrated: `table.info_store_read_view().interner_read_view().await` → `InternerReadView::inner_to_query_value(&reopened)` |

Persist-once assertion (`FJALL_PERSIST_SYNC_ALL_COUNT == 1`) and the
"second persist is a no-op fast path" semantics are unchanged — the
persist call still goes to the same `Interner::persist`.

### Re-grep (all demoted names, crates/*/tests + benches + examples)

- `add_table`: only `RepoConfig::new(...).add_table(...)` builder chains
  remain (shamir-db/shamir-server tests+benches) — PUBLIC builder, unaffected.
- `append` / `append_batch`: only other types — server audit `chain.append`
  (audit_appender.rs:23, audit_rotation.rs:134), std `Command::append(true)`
  (seed_recovery_e2e/harness/child.rs:122), WAL `gc.append` /
  `set.append_batch` (shamir-wal benches, `WalGroupCommit`/`SegmentSet`).
- All other names on the sweep list: zero hits in tests/benches/examples.
- RepoInstance receivers: zero remaining.

### Notes / unverified

- No cargo/fmt/clippy run (per brief). Compile-readiness by re-read: every
  migrated site drops the owned `RepoWritePermit` before any later call that
  could re-admit (`get_table` cold path, DDL, `flush_buffers`); no permit
  guard is held across a nested `admit().await`.
- tx_pipeline `make_repo` builds a throwaway current-thread runtime per call
  (7 call sites, all sync) — setup-only cost, outside all timed sections.
- `intern_query_value_admitted` vs the old inline `query_value_to_inner`:
  identical interning semantics (same codec fn under `debug_check_permit`).

## Round 11 addendum (items 1-2)

### Item 1 — wrong-domain permit panics in `access_meta_tests` (4 tests)

Tests: `resource_meta_fails_closed_on_storage_error`,
`authorize_access_denies_when_resource_meta_errors`,
`resource_meta_group_fails_closed_on_storage_error`,
`authorize_access_denies_when_group_resource_meta_errors`
(`crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs`).

- Mechanism (pre): the fixtures `shamir_with_failing_databases_table`
  (access_meta_tests.rs:741) and `shamir_with_failing_groups_table`
  (access_meta_tests.rs:836) built the fault-injecting replacement table via
  `TableManager::create` (access_meta_tests.rs:750 / 845).
  `TableManager::create` mints a **fresh private `RepoWriteDomain`**
  (`crates/shamir-engine/src/table/table_manager.rs:581-587`,
  `RepoWriteDomain::new(name)`), then `install_table_for_test`
  (`crates/shamir-engine/src/repo/repo_instance.rs:554`) spliced that table
  into the SYSTEM_REPO, whose tables live under the repo's OWN domain
  (domain id 1). Production reads on that table presented a permit admitted
  on the repo's domain (id 1) to a table whose `debug_check_permit` checks
  against the table's domain (id 2) → panic
  `permit was issued by a different RepoWriteDomain instance` (left: 1,
  right: 2).
- Root cause: the FIXTURE, not production code. Production admits on the
  same instance whose tables it touches; the fixture constructed a
  second domain instance for the injected table.
- Fix (post): both fixtures now create the replacement table with
  `TableManager::create_with_domain(..., Arc::clone(system_repo.write_domain()))`
  (access_meta_tests.rs:750-767 and 853-870), so the injected table lives in
  the same `RepoWriteDomain` instance that owns the repo it is installed
  into. No assertion weakened; `debug_assert` untouched.

### Item 2 — `stale_name_collision_is_rejected_not_silently_no_oped`

Test: `crates/shamir-db/src/shamir_db/shamir_db/tests/schema_rollback_tests.rs:341`.

- Mechanism (pre): PR6b-2a rewrote `ShamirDb::compile_table_schema`
  (`crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs:503`) to
  resolve the repository and admit on its write domain BEFORE calling
  `compile_table_schema_admitted`, which is where the F-24 stale-name
  collision check (`ValidatorRegistry::replace_artifact` returning false →
  "stale name collision") lived. On master, the collision check ran BEFORE
  any repo resolution (repo lookup only happened later, inside activation,
  where its failure was tolerated). The test plants a validator under a
  name for a ghost repo (`testdb.main` never created) and retries with a
  different id — the new admission-first order returned
  `Item not found: Repository 'testdb.main' not found` before the collision
  check could run.
- Constraint: the sibling test
  `compile_table_schema_waits_for_repository_write_admission`
  (schema_rollback_tests.rs:118) requires that the fresh-registration
  `register` happens only AFTER admission (registry must be unmutated while
  admission blocks), so the whole registry step could not simply be hoisted
  above the admit.
- Fix (post, schema_management.rs:503-601): `compile_table_schema` now
  - computes `freshly_registered` / `prior_artifact` (pure registry reads),
  - in the ALTER branch (`!freshly_registered`), runs the stale-collision
    `replace_artifact` check BEFORE the repo lookup/admission — restoring
    master's observable error for the ghost-repo case — and restores the OLD
    artifact if the subsequent lookup/admit fails (F-27b restore-on-failure),
  - keeps the FRESH `register` inside the admission-gated
    `compile_table_schema_admitted` (admission-wait guarantee preserved),
  - performs the wrapper-level symmetric rollback (fresh → `remove`,
    ALTER → restore OLD artifact) when the admitted call fails, superseding
    the admitted fn's new-for-new restore with the true OLD artifact.
  No assertion weakened; `debug_assert` untouched.

## Round 11 addendum (items 3-4)

### Item 3 — `reopened_checkpointed_truncated_history_allows_next_commit` (`Storage("FjallError: Locked")` on first reopen)

Test: `crates/shamir-engine/src/repo/durability_policy/tests/power_loss_bound_tests/reopen_tests.rs:126`
(`open()`'s un-retried `.unwrap()` at `reopen_tests.rs:31`).

- Mechanism (pre): the fjall file lock lives as long as ANY `Arc<Database>`
  clone. Every `FjallStore` spawns a write-worker thread that OWNS a clone of
  the keyspace + database handle
  (`crates/shamir-storage/src/storage_fjall.rs:118`, `WriteWorker::spawn`;
  the lock is released only when the owning store drops and
  `impl Drop for WriteWorker` at `storage_fjall.rs:158` closes the channel and
  joins the thread). The test's `drop(repo)` therefore releases the lock only
  when EVERY handle is gone — including the handles pinned by the one task
  that deterministically outlives the drop: the background DRAINER task, which
  owns a full background `RepoInstance` clone
  (`crates/shamir-engine/src/repo/repo_instance.rs` `drainer()` →
  `drainer.spawn(self.clone_for_background(), ...)`; task body
  `crates/shamir-engine/src/tx/drainer.rs` `Drainer::spawn`). That loop only
  OBSERVES foreground-liveness death at its park boundaries
  (`drainer.rs:1666-1687`: `live.upgrade().is_none()` checked before/after a
  `select!` park whose backstop is the full 50 ms `DRAIN_INTERVAL_MS`,
  `repo_instance.rs:385`). So the drainer's background clone — and with it
  every fjall store handle — survives up to one full park interval past
  `drop(repo)`; the PR's admission-gated close-path work (permits threaded
  through `from_factory`, `get_table`'s self-admission, admitted recovery
  twins) lengthens the final in-flight tick, pushing the release past the
  test's immediate, UN-retried first reopen → deterministic
  `Storage("FjallError: Locked")`. The test's own doc
  ("no production task leaks the instance", `reopen_tests.rs:66-71`) states
  the invariant the loop only APPROXIMATED.
- Fix (post, proper async close path — no sleeps, no test edits):
  - `repo_instance.rs`: new `RepoDropSignal` (`notify: tokio::sync::Notify`
    + `Drop` that calls `notify_one()` — a SYNC method, legal in `Drop`);
    paired `drop_signal: Option<Arc<RepoDropSignal>>` field mirrors `live`
    (`Some` on foreground clones, `None` on `clone_for_background`), cloned in
    `impl Clone for RepoInstance`, initialized in the constructor, and its
    `Notify` handed to `Drainer::spawn` in `drainer()`.
  - `drainer.rs` `spawn`: new `drop_notify: Option<tokio::sync::Notify>`
    parameter; the park's `select!` gains a third branch awaiting the notify
    (`std::future::pending` when `None`). The last foreground clone's `Drop`
    now wakes the parked loop immediately → the post-park
    `live.upgrade().is_none()` exit check runs without waiting out the 50 ms
    backstop → the background clone (and its fjall handles) is released
    deterministically, restoring master's plain-drop release semantics.
  No assertion weakened; no timeout touched; the 50 ms backstop is unchanged
  for drain purposes.

### Item 4 — `p03b_index2_name_reuse_rejected_during_drop` (600 s timeout, deadlock)

Test: `crates/shamir-engine/src/table/tests/p03b_index2_drop_durability_tests.rs:409`.

- Mechanism (pre): `drop_index2` (`table_manager_index_mgmt.rs:303-306`)
  admits the repo permit and `drop_index2_admitted`
  (`write_receipt/admission/write_domain_index_core.rs:312+`) holds it across
  the ENTIRE critical section — including the F-76 pre-sweep test-hook park
  (`write_domain_index_core.rs:372-381`, `drop_index2_pause_hook`) and the
  reader-drain wait (`:396`, `drain_guard.wait_for_drain()`). The test drops
  in a background task which parks at the hook; the MAIN task then calls
  `mgr.create_index_v2(...)` (`:443`), whose PR6b-2a wrapper
  (`table_manager_index_mgmt.rs:34-40`) acquires the repo permit FIRST —
  before the P0-3b name-reuse fast-reject — so it parks forever on the
  non-reentrant domain mutex held by the parked drop. The only task that can
  release the hook is the SAME parked task → circular wait → the test's
  600 s bound. (Same failure class as the pre-PR #1012 fix recorded in the
  engineering log: a guard acquired before the tombstone fast-reject check
  reintroduces the P0-3b deadlock one layer up — here the REPO permit instead
  of the per-table `ddl_admission`.)
- Fix (post):
  - `table_manager_index_mgmt.rs`: new
    `TableManager::reject_create_while_drop_in_flight` (pure reads of the
    persisted drop tombstone + metadata — safe WITHOUT admission); the
    `create_index_v2` wrapper runs it BEFORE `self.admit()` for every
    non-btree index type, so a CREATE racing a drop parked at its hook
    rejects immediately and the main task reaches `hook.release()`.
  - `write_receipt/admission/write_domain_index_admitted.rs`:
    `create_index_v2_admitted` now calls the same shared helper in place of
    its inline tombstone scan (identical rejection message/semantics;
    admitted callers get the same fast-reject). The check remains cheap-pure;
    the admit→check race window is unchanged in strength: a CREATE that
    passes the pre-check and then blocks on admission still re-serializes
    behind the in-flight drop, exactly as the R0-A comment documents.
  Lock order preserved: permit → marker → per-table DDL admission → barrier →
  `bindings_lock`/`persist_lock`; no permit is held across the drain wait
  beyond the pre-existing drop critical section. No assertion weakened; the
  test and its timeout config untouched.

## Round 11 addendum (item 5)

Tests: `nested_batch_alter_is_audited` and
`foreach_execution_audits_each_alter_iteration`
(`crates/shamir-server/src/db_handler/tests/receipts/repo_audit/tests.rs:189`
and `:206`) — SIGABRT with stack overflow, two levels of nested
`Batch` → `ALTER REPO`.

- Mechanism (pre): both tests drive `ShamirDbHandler::handle` → `execute`
  → engine `QueryRunner::run` (`crates/shamir-engine/src/query/batch/query_runner.rs:1086`),
  which recurses into nested `Batch`/`ForEach` bodies and dispatches admin
  ops through `ShamirAdminExecutor::execute_admin`
  (`crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs:20`). This PR's
  S5c-X work grew the shared `execute_admin` match with the `AlterRepo` arm
  (`admin_dispatch.rs:38`) whose `handle_alter_repo` future
  (`crates/shamir-db/src/shamir_db/execute/repo_admin/admin_repo_durability.rs:59`)
  embeds the whole durability path's await state: authorize_access, the
  per-db `repo_create_locks` mutex acquire, the system-store
  `load_repository_record`, the `save_repository` catalogue persist and the
  durability swap/observer. Because every arm of the match is an inline
  `.await`, `execute_admin`'s own future is the SUM of all arm futures — and
  that sum is re-materialised in every frame of the nested-batch/ForEach
  recursion → Windows test-thread stack overflow. Same mechanism as the
  Round-14 replication fix (commit `dcac3be1`, "keep grown replication admin
  futures off the dispatch frame").
- Fix (post): the precedent's pattern applied to the ALTER REPO path.
  - `admin_dispatch.rs:38-43`: the `BatchOp::AlterRepo` arm no longer
    awaits `handle_alter_repo` inline; it routes through
    `Self::dispatch_alter_repo(self, op).await`.
  - `admin_dispatch.rs:135-155`: new
    `type AlterRepoDispatchFuture<'a> = Pin<Box<dyn Future<Output =
    Result<QueryResult, BatchError>> + Send + 'a>>` and
    `#[inline(never)] fn dispatch_alter_repo` which `Box::pin`s the
    `handle_alter_repo` call — the durability path's await state now lives
    on the heap and appears ONCE per dispatch instead of inside every
    `execute_admin` state machine along the recursion.
  - `admin_repo_durability.rs:97-102` and `:150-166`: the handler's two
    heaviest inline awaits — `load_repository_record` and
    `save_repository` — are `Box::pin`'ed, shrinking the (now boxed)
    handler future itself.
  No behavior change, no assertion edits, no stack-size changes; dispatch
  order and error shapes are untouched.
