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
