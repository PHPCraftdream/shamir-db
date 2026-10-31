use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::KvOp;
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_keys::{
    build_index_key, build_index_key_from_record, build_posting_key, canonical_from_values,
    is_digest_form,
};
use crate::base_index::index_record_key::{
    is_legacy_base_posting_key, posting_index_prefix, IndexRecordKey,
};
use crate::model::identity::identity_bytes::posting_tail_matches;
use crate::model::identity::markers::DIGEST_MARKER;
use crate::write_ops::IndexWriteOp;
use crate::IndexState;

use super::inner::{regular_provenance, IndexManager};

impl IndexManager {
    /// Обработчик события создания записи.
    ///
    /// Добавляет новую запись во все активные индексы.
    /// Вызывается после успешной вставки записи в таблицу.
    ///
    /// # Аргументы
    ///
    /// * `record_id` — идентификатор новой записи
    /// * `value` — значение записи
    pub async fn on_record_created(
        &self,
        record_id: &RecordId,
        value: &InnerValue,
    ) -> DbResult<()> {
        let ops = self.plan_record_created(record_id, value).await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_record_created` — returns
    /// `Vec<IndexWriteOp>` instead of writing directly to `info_store`.
    pub async fn plan_record_created(
        &self,
        record_id: &RecordId,
        value: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let defs = self.indexes.snapshot();
        let mut ops = Vec::with_capacity(4);
        for def in defs.iter() {
            // #1058: if this def is Building AND has an active in-flight build,
            // capture RecordId to dirty-set instead of producing SetPosting.
            let in_flight =
                def.state == IndexState::Building && self.is_build_in_flight(def.name_interned);

            if let Some((irk, canonical)) =
                build_index_key_from_record(false, def.name_interned, value, &def.paths)
            {
                if in_flight {
                    // Capture to dirty-set — no SetPosting for this def.
                    if let Some(dirty_set) = self.get_or_create_dirty_set(def.name_interned) {
                        let mut set = dirty_set.lock().unwrap();
                        set.insert(*record_id);
                    }
                } else {
                    // Normal path: produce SetPosting for Ready defs (or Building
                    // defs not yet in-flight-registered — those still need direct
                    // writes for the "delta catch-up for free" mechanism before
                    // Phase B starts). Value: empty for the inline form, the
                    // canonical bytes `C` for the digest form (design (a)).
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    let value = if is_digest_form(&canonical) {
                        Bytes::from(canonical)
                    } else {
                        Bytes::new()
                    };
                    ops.push(IndexWriteOp::SetPosting {
                        key: posting_key,
                        value,
                        provenance: regular_provenance(def),
                    });
                }
            }
        }

        Ok(ops)
    }

    /// Batched version of `on_record_created`. Accepts borrowed
    /// (id, &value) pairs to avoid cloning N `InnerValue`s
    /// (`InnerValue::Map` clones its full nested structure — costly
    /// for wide records).
    ///
    /// All posting writes across all regular indexes are collected
    /// into ONE `Store::set_many` call → one backend commit on
    /// transactional backends, same number of individual writes on
    /// default-loop backends but with reduced allocation overhead.
    pub async fn on_records_created_batch<'a, R, I>(&self, items: I) -> DbResult<()>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        let ops = self.plan_records_created_batch(items).await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_records_created_batch` — returns
    /// accumulated `Vec<IndexWriteOp>` for all items across all
    /// regular indexes.
    pub async fn plan_records_created_batch<'a, R, I>(
        &self,
        items: I,
    ) -> DbResult<Vec<IndexWriteOp>>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::with_capacity(1024);
        let defs = self.indexes.snapshot();
        for def in defs.iter() {
            let provenance = regular_provenance(def);
            let in_flight =
                def.state == IndexState::Building && self.is_build_in_flight(def.name_interned);
            for (rid, value) in items.clone() {
                if let Some((irk, canonical)) =
                    build_index_key_from_record(false, def.name_interned, value, &def.paths)
                {
                    if in_flight {
                        if let Some(dirty_set) = self.get_or_create_dirty_set(def.name_interned) {
                            dirty_set.lock().unwrap().insert(*rid);
                        }
                        continue;
                    }
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, rid);
                    let value = if is_digest_form(&canonical) {
                        Bytes::from(canonical)
                    } else {
                        Bytes::new()
                    };
                    ops.push(IndexWriteOp::SetPosting {
                        key: posting_key,
                        value,
                        provenance,
                    });
                }
            }
        }

        Ok(ops)
    }

    /// Обработчик события обновления записи.
    ///
    /// Обновляет индексы при изменении записи:
    /// - Если проиндексированные поля не изменились — ничего не делает
    /// - Если изменились — удаляет старые записи индекса и добавляет новые
    ///
    /// # Аргументы
    ///
    /// * `record_id` — идентификатор обновлённой записи
    /// * `old_value` — старое значение (до обновления)
    /// * `new_value` — новое значение (после обновления)
    pub async fn on_record_updated(
        &self,
        record_id: &RecordId,
        old_value: &InnerValue,
        new_value: &InnerValue,
    ) -> DbResult<()> {
        let ops = self
            .plan_record_updated(record_id, old_value, new_value)
            .await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_record_updated` — returns
    /// `RemovePosting` for removed values + `SetPosting` for added.
    pub async fn plan_record_updated(
        &self,
        record_id: &RecordId,
        old_value: &(impl RecordRef + ?Sized),
        new_value: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::with_capacity(4);
        let defs = self.indexes.snapshot();
        for def in defs.iter() {
            // #1058: if this def is Building AND has an active in-flight build,
            // capture RecordId to dirty-set instead of producing posting ops.
            let in_flight =
                def.state == IndexState::Building && self.is_build_in_flight(def.name_interned);

            let provenance = regular_provenance(def);
            let old_key =
                build_index_key_from_record(false, def.name_interned, old_value, &def.paths);
            let new_key =
                build_index_key_from_record(false, def.name_interned, new_value, &def.paths);

            if in_flight {
                // Capture to dirty-set if the write touches this index's fields.
                // We capture for ANY key change (including None→None, though that
                // is a no-op — the dirty-set handles it fine as an over-inclusive
                // but harmless entry).
                let touches_index = old_key.is_some() || new_key.is_some();
                if touches_index {
                    if let Some(dirty_set) = self.get_or_create_dirty_set(def.name_interned) {
                        let mut set = dirty_set.lock().unwrap();
                        set.insert(*record_id);
                    }
                }
            } else {
                // Normal path: produce RemovePosting/SetPosting as usual.
                match (old_key, new_key) {
                    (None, None) => {}
                    (None, Some((nk, nc))) => {
                        let index_key = nk.to_bytes();
                        let posting_key = build_posting_key(&index_key, record_id);
                        let value = if is_digest_form(&nc) {
                            Bytes::from(nc)
                        } else {
                            Bytes::new()
                        };
                        ops.push(IndexWriteOp::SetPosting {
                            key: posting_key,
                            value,
                            provenance,
                        });
                    }
                    (Some((ok, _oc)), None) => {
                        let index_key = ok.to_bytes();
                        let posting_key = build_posting_key(&index_key, record_id);
                        ops.push(IndexWriteOp::RemovePosting {
                            key: posting_key,
                            provenance,
                            owner: None,
                        });
                    }
                    (Some((ok, oc)), Some((nk, nc))) => {
                        let old_bytes = ok.to_bytes();
                        let new_bytes = nk.to_bytes();
                        if old_bytes != new_bytes || oc.as_slice() != nc.as_slice() {
                            let old_posting_key = build_posting_key(&old_bytes, record_id);
                            ops.push(IndexWriteOp::RemovePosting {
                                key: old_posting_key,
                                provenance,
                                owner: None,
                            });

                            let new_posting_key = build_posting_key(&new_bytes, record_id);
                            let value = if is_digest_form(&nc) {
                                Bytes::from(nc)
                            } else {
                                Bytes::new()
                            };
                            ops.push(IndexWriteOp::SetPosting {
                                key: new_posting_key,
                                value,
                                provenance,
                            });
                        }
                    }
                }
            }
        }

        Ok(ops)
    }

    /// Обработчик события удаления записи.
    ///
    /// Удаляет запись из всех активных индексов.
    /// Вызывается после успешного удаления записи из таблицы.
    ///
    /// # Аргументы
    ///
    /// * `record_id` — идентификатор удалённой записи
    /// * `old_value` — значение удалённой записи
    pub async fn on_record_deleted(
        &self,
        record_id: &RecordId,
        old_value: &InnerValue,
    ) -> DbResult<()> {
        let ops = self.plan_record_deleted(record_id, old_value).await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_record_deleted` — returns
    /// `RemovePosting` for each posting of this record.
    pub async fn plan_record_deleted(
        &self,
        record_id: &RecordId,
        old_value: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::with_capacity(4);
        let defs = self.indexes.snapshot();
        for def in defs.iter() {
            // #1058: if this def is Building AND has an active in-flight build,
            // capture RecordId to dirty-set instead of producing RemovePosting.
            let in_flight =
                def.state == IndexState::Building && self.is_build_in_flight(def.name_interned);

            if let Some((irk, _canonical)) =
                build_index_key_from_record(false, def.name_interned, old_value, &def.paths)
            {
                if in_flight {
                    // Capture to dirty-set — no RemovePosting for this def.
                    if let Some(dirty_set) = self.get_or_create_dirty_set(def.name_interned) {
                        let mut set = dirty_set.lock().unwrap();
                        set.insert(*record_id);
                    }
                } else {
                    // Normal path: produce RemovePosting for Ready defs (or Building
                    // defs not yet in-flight-registered).
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    ops.push(IndexWriteOp::RemovePosting {
                        key: posting_key,
                        provenance: regular_provenance(def),
                        owner: None,
                    });
                }
            }
        }

        Ok(ops)
    }

    // ============================================================================
    // Apply ops — shared by all wrapper methods
    // ============================================================================

    /// Apply a slice of `IndexWriteOp` against `self.info_store`.
    /// Used by the `on_record_*` wrapper methods after planning.
    ///
    /// All SetPosting/RemovePosting ops are collapsed into ONE
    /// ordered `Store::transact` call — on transactional backends
    /// (sled / redb / fjall / persy / nebari / canopy) this is one
    /// atomic batch (one fsync) instead of N per-key writes. Order is
    /// preserved, so the per-key last-write-wins semantics of the
    /// original loop are unchanged. BumpFtsStats is in-memory only
    /// and not relevant for the base_index IndexManager.
    pub(in crate::base_index) async fn apply_ops(&self, ops: &[IndexWriteOp]) -> DbResult<()> {
        let mut kv_ops: Vec<KvOp> = Vec::with_capacity(ops.len());
        for op in ops {
            match op {
                IndexWriteOp::SetPosting { key, value, .. } => {
                    kv_ops.push(KvOp::Set(key.clone().into(), value.clone()));
                }
                IndexWriteOp::RemovePosting { key, .. } => {
                    kv_ops.push(KvOp::Remove(key.clone().into()));
                }
                IndexWriteOp::BumpFtsStats { .. } => {
                    // Not relevant for base_index IndexManager.
                }
            }
        }
        // Bump the cache epoch BEFORE mutating the store: any scan that
        // sampled the pre-bump epoch reflects pre-mutation state valid at
        // bump time, so publishing it can never outlive this invalidation.
        self.posting_cache_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if !kv_ops.is_empty() {
            self.info_store.transact(kv_ops).await?;
        }
        // Invalidate posting cache for any keys touched.
        self.invalidate_posting_cache_for_ops(ops);
        Ok(())
    }

    /// Ищет записи по значению индекса.
    ///
    /// Возвращает множество RecordId, у которых проиндексированные поля
    /// соответствуют указанным значениям.
    ///
    /// # Аргументы
    ///
    /// * `name_interned` — интернированный идентификатор имени индекса
    /// * `values` — значения для поиска (должны соответствовать полям индекса)
    ///
    /// # Возвращает
    ///
    /// - `Ok(Arc<[RecordId]>)` — сортированный, дедуплицированный срез
    ///   идентификаторов записей за рефкаунт-дугой (O(1) копирование —
    ///   см. ниже). Инвариант: элементы отсортированы по `RecordId::Ord`
    ///   (байт-лексикографически) и не содержат дубликатов.
    /// - `Ok(empty slice)` — если нет записей с таким значением индекса
    /// - `Err` — ошибка чтения из хранилища
    ///
    /// # Audit 1.5 + 3.2 — Arc-срез вместо `BTreeSet`
    ///
    /// Раньше на HIT в `posting_cache` делался полный node-by-node клон
    /// дерева на КАЖДЫЙ equality-lookup (для низкокардинального индекса с
    /// 100k постингов это 100k аллокаций узлов); задача #488 заменила это
    /// на `Arc<BTreeSet<_>>` (HIT = O(1) `Arc::clone`). Задача #499 меняет
    /// само ПРЕДСТАВЛЕНИЕ: `Arc<[RecordId]>` вместо `Arc<BTreeSet<_>>`.
    ///
    /// Префикс-скан по `index_key` уже отдаёт постинги в порядке
    /// возрастания хвостовых байт `record_id` — а `RecordId::Ord` и есть
    /// байт-лексикографический порядок над `[u8; 16]`, — и ровно по одному
    /// постингу на `(index_key, record_id)`. Значит скан уже отсортирован и
    /// дедуплицирован: `BTreeSet::insert` на каждый элемент был чистыми
    /// накладными расходами (аллокация узла + ребалансировка). Теперь
    /// MISS-путь собирает `Vec<RecordId>` в порядке скана (push, без
    /// per-node аллокаций) и оборачивает в `Arc<[RecordId]>`, а потребитель
    /// итерирует непрерывный кэш-дружелюбный буфер вместо погони за
    /// указателями дерева.
    pub async fn lookup_by_index(
        &self,
        name_interned: u64,
        values: &[InnerValue],
    ) -> DbResult<Option<Arc<[RecordId]>>> {
        // P0-3a (#1011): acquire the reader-drain guard as the FIRST statement
        // — before `build_index_key`, before the posting-cache probe, before
        // anything that touches the shared `info_store` keyspace. `None` means a
        // DROP INDEX is currently in its raise→sweep window: the caller MUST
        // NOT read the physical store and MUST fall back (a full table scan, or
        // an explicit `DbError::NotFound` for the introspection-by-name API) —
        // collapsing `None` into an empty `Some` would silently convert the
        // rare "incomplete" into a rare "wrongly empty", which is strictly
        // worse than the pre-fix status quo. See `crate::reader_drain_gate`'s
        // module doc for the placement invariant: do not take outer DDL or
        // lifecycle locks while holding the guard. Leaf cache shard locks do
        // not wait on the drain gate and are released before asynchronous work.
        let Some(_guard) = self.reader_gate.enter() else {
            return Ok(None);
        };

        // A probe with no identity (NaN) matches nothing: no posting can
        // exist for it (NaN never produces canonical bytes, inline or
        // digested), so return the empty result directly.
        let Some(index_key) = build_index_key(false, name_interned, values) else {
            return Ok(Some(Arc::from(Vec::new())));
        };
        let index_key = index_key.to_bytes();
        // Digest-form probes verify each posting's canonical tail (design
        // (a)): the key is only a locator, two distinct long values sharing
        // a BLAKE3 digest each keep their own postings distinguished by the
        // trailing rid.
        let probe_canonical = canonical_from_values(values);
        let probe_is_digest = index_key.get(9) == Some(&DIGEST_MARKER);
        // A digest-form key is only a LOCATOR: two distinct long values that
        // collide on the BLAKE3 digest share the same cache slot but have
        // different true result sets, so digest-form lookups bypass the
        // posting cache entirely (correctness over the tiny hit-rate cost).

        // Opt G: try the in-memory posting cache first. DashMap's
        // sharded RwLock lets unrelated concurrent lookups proceed
        // without serialising on a single mutex.
        //
        // P0-3a (#1011): this cache probe MUST be INSIDE the guard's scope
        // (i.e. after `enter`). A racing reader that populated the cache with a
        // PARTIAL result AFTER a concurrent sweep's `posting_cache.retain` ran
        // would otherwise poison the cache with an entry that outlives the DROP
        // and (same `name_interned` → same physical prefix) gets re-served if
        // the index name is re-created — finding C-5 in the plan doc.
        // Audit 1.5: cache-HIT — это атомарный refcount-bump через
        // `Arc::clone`, а НЕ полный node-by-node клон `BTreeSet`.
        // HIT is served only when the entry's scan generation still matches
        // the current shared epoch; a writer bump invalidates every older tag.
        let generation = self
            .posting_cache_generation
            .load(std::sync::atomic::Ordering::Acquire);
        if let Some(cached) = if probe_is_digest {
            None
        } else {
            self.posting_cache.get(&index_key)
        } {
            let (cached_generation, values) = cached;
            if cached_generation == generation
                && self
                    .posting_cache_generation
                    .load(std::sync::atomic::Ordering::Acquire)
                    == generation
            {
                return Ok(Some(values));
            }
        }

        // P0-3a (#1011) test seam: park here (guard HELD, scan not yet started)
        // so a regression test can deterministically hold a read mid-flight
        // inside the drain window. NOT `#[cfg(test)]`-gated — cross-crate test
        // consumer, same reason as `drop_index_pause_hook`. `None` on every real
        // path — one uncontended `ArcSwapOption::load_full()` Acquire load.
        if let Some(hook) = self.lookup_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Capture before scanning; cache hits and publication validate this
        // shared epoch so invalidation cannot leave an old scan reusable.
        let scan_generation = self
            .posting_cache_generation
            .load(std::sync::atomic::Ordering::Acquire);

        // Scan the `9 + |I|`-byte index prefix; every match is a posting
        // entry whose final 16 bytes are the record_id. The scan visits
        // postings in ascending order of the trailing record_id bytes,
        // which is exactly `RecordId::Ord` (byte-lexicographic over
        // `[u8; 16]`), and there is at most one posting per record_id —
        // so `record_ids` is built already-sorted and duplicate-free by
        // plain `push`, no `BTreeSet` insert/rebalance needed (audit 3.2).
        // The key length is checked EXACTLY (`== prefix + 16`): a longer or
        // shorter key under the prefix is not a posting of this identity.
        // tunables: one-off prefix-scan batch (512); fold into a named knob under /opti.
        let mut record_ids: Vec<RecordId> = Vec::new();
        let mut stream = self.info_store.scan_prefix_stream(index_key.clone(), 512);
        while let Some(batch) = stream.next().await {
            for (k, v) in batch? {
                let kb: &[u8] = k.as_ref();
                if kb.len() == index_key.len() + 16 {
                    // Digest form: drop postings whose canonical tail
                    // differs from the probe's `C` (a BLAKE3 collision of
                    // two distinct values must never surface as a hit).
                    if probe_is_digest {
                        let matches = probe_canonical
                            .as_deref()
                            .is_some_and(|probe_c| posting_tail_matches(v.as_ref(), probe_c));
                        if !matches {
                            continue;
                        }
                    }
                    let mut id_bytes = [0u8; 16];
                    id_bytes.copy_from_slice(&kb[index_key.len()..index_key.len() + 16]);
                    record_ids.push(RecordId(id_bytes));
                }
            }
        }

        // Deterministic regression seam for the scan-to-publication race.
        if let Some(hook) = self.lookup_post_scan_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        let record_ids_arc: Arc<[RecordId]> = Arc::from(record_ids);
        // Validate the scan's generation in the SAME atomic cell writers bump:
        // publication proceeds only while the epoch is still the sampled one,
        // closing the check-before-insert gap without a per-key tombstone map.
        let publish_allowed = self
            .posting_cache_generation
            .compare_exchange(
                scan_generation,
                scan_generation,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok();
        if publish_allowed && !probe_is_digest {
            self.posting_cache
                .insert(index_key, scan_generation, Arc::clone(&record_ids_arc));
        }

        Ok(Some(record_ids_arc))
    }

    /// Invalidate posting cache entries for every `SetPosting` /
    /// `RemovePosting` in `ops`. Called by the tx-commit pipeline
    /// (`apply_index_batch`) after durably writing the ops, so the
    /// next `lookup_by_index` re-fetches from the store. The index-key
    /// prefix is derived from the posting's namespace byte (identity v2:
    /// regular = key minus the 16-byte rid tail, unique = whole key;
    /// legacy v1 = the fixed 25-byte prefix) — see
    /// `posting_index_prefix`.
    pub fn invalidate_posting_cache_for_ops(&self, ops: &[IndexWriteOp]) {
        let mut touched = false;
        for op in ops {
            let key = match op {
                IndexWriteOp::SetPosting { key, .. } | IndexWriteOp::RemovePosting { key, .. } => {
                    key
                }
                _ => continue,
            };
            let index_key = posting_index_prefix(key.as_ref())
                .map(Bytes::copy_from_slice)
                .or_else(|| is_legacy_base_posting_key(key.as_ref()).then(|| key.slice(..25)));
            if let Some(index_key) = index_key {
                self.posting_cache.remove(&index_key);
                touched = true;
            }
        }
        if touched {
            self.posting_cache_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    /// Invalidate posting cache entries for raw posting keys written
    /// straight onto `info_store` (the drainer's WAL index replay), which
    /// bypasses [`apply_ops`](Self::apply_ops) and its invalidation. Call it
    /// AFTER the write lands so no reader can re-cache the old state between
    /// the two. Keys shorter than the index prefix are ignored.
    pub fn invalidate_posting_cache_for_keys<'a>(&self, keys: impl IntoIterator<Item = &'a [u8]>) {
        let mut touched = false;
        for key in keys {
            let index_key = posting_index_prefix(key)
                .map(Bytes::copy_from_slice)
                .or_else(|| {
                    is_legacy_base_posting_key(key).then(|| Bytes::copy_from_slice(&key[..25]))
                });
            if let Some(index_key) = index_key {
                self.posting_cache.remove(&index_key);
                touched = true;
            }
        }
        if touched {
            self.posting_cache_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    /// Count entries for one regular or unique index — used by the
    /// doctor's verify pass.
    pub async fn entry_count(&self, name_interned: u64, unique: bool) -> DbResult<u64> {
        let prefix = IndexRecordKey::new(unique, name_interned).to_prefix_bytes();
        let mut count: u64 = 0;
        let stream = self.info_store.scan_prefix_stream(prefix, 1024);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            count += batch?.len() as u64;
        }
        Ok(count)
    }

    pub fn iter_indexes(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.indexes.iter()
    }

    /// Planner Ready-gate sibling of [`iter_indexes`](Self::iter_indexes):
    /// yields only `Ready` definitions, skipping any `Building` one exactly
    /// as if it did not exist yet.
    ///
    /// F-72 (#899, P0): closes the planner-invisibility gap for the regular
    /// (non-unique) hash-index family — `TableManager::find_single_field_index`
    /// and `try_plan_and_index_scan` (`read_planner.rs`) use this instead of
    /// the raw `iter_indexes`, so a concurrent Eq/In/And query cannot be
    /// planned against an index whose backfill has not yet completed (see
    /// `create_index_from_records`'s doc for the full publish/backfill/flip
    /// sequence this gates).
    pub fn iter_indexes_ready(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.indexes
            .iter()
            .filter(|d| d.state == crate::state::IndexState::Ready)
    }

    /// Проверяет существование индекса по его имени.
    pub fn index_exists(&self, name_interned: u64) -> bool {
        self.indexes.contains(name_interned)
    }

    /// Возвращает определение индекса по его имени.
    pub fn get_index_definition(&self, name_interned: u64) -> Option<IndexDefinition> {
        self.indexes.get_index(name_interned)
    }

    /// Re-key an in-memory regular-index definition from `old_id` to `new_id`
    /// and persist the updated metadata.
    ///
    /// This is the metadata half of RENAME INDEX — the physical posting
    /// entries are re-keyed separately by the engine (`rekey_hash_prefix`)
    /// before this method is called. Here we only swap the in-memory
    /// `IndexDefinition` and re-save the system blob.
    ///
    /// F-8 (2026-08-06): currently unused — the live regular-index RENAME
    /// path (`TableManager::rename_index`) does drop-old+create-new instead,
    /// which bumps `generation` as a side effect of going through the normal
    /// create/drop call sites. If this method is ever wired up as a cheaper
    /// in-place rename, `bump_generation()` below is required: without it,
    /// `pre_commit.rs`'s `mgr.generation() == stage_gen` gate would not fire
    /// for a tx staged before the rename, silently skipping re-derivation.
    pub async fn rename_index_definition(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let old_def = self.indexes.get_index(old_id).ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "index definition disappeared mid-rename".to_string(),
            )
        })?;
        self.indexes.remove_index(old_id);
        let new_def = IndexDefinition::new(new_id, old_def.paths.clone());
        self.indexes.add_index(new_def);
        self.bump_generation(); // F-8: gen gate for commit-time rederive
        self.save_index_info().await
    }

    /// Re-key an in-memory unique-index definition from `old_id` to `new_id`
    /// and persist the updated metadata.
    ///
    /// Metadata half of RENAME INDEX for unique indexes — the physical
    /// posting entries are re-keyed separately by the engine
    /// (`rekey_hash_prefix` with `is_unique=true`).
    ///
    /// F-8 (2026-08-06): same "currently unused, bump required if ever
    /// wired up" note as [`rename_index_definition`](Self::rename_index_definition).
    pub async fn rename_unique_index_definition(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let old_def = self.indexes_unique.get_index(old_id).ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "unique index definition disappeared mid-rename".to_string(),
            )
        })?;
        self.indexes_unique.remove_index(old_id);
        let new_def = IndexDefinition::new(new_id, old_def.paths.clone());
        self.indexes_unique.add_index(new_def);
        self.bump_generation(); // F-8: gen gate for commit-time rederive
        self.save_index_info_unique().await
    }
}
