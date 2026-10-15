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
