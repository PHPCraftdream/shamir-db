use bytes::Bytes;
use smallvec::SmallVec;

use super::covering::{build_covering_projection, decode_sorted_index_name, extract_and_encode};
use super::SortedIndexDefinition;
use crate::write_ops::IndexWriteOp;
use shamir_storage::error::DbResult;
use shamir_storage::types::RecordKey;
use shamir_types::core::interner::Interner;
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;

impl super::inner::SortedIndexManager {
    // ============================================================================
    // Covering-index helpers
    // ============================================================================

    /// Resolve `included_fields` string paths to interned u64 ids for every
    /// definition that has at least one included field. Call this:
    ///   1. After `register()` when the caller already has an interner, OR
    ///   2. After construction (load from disk) to rebuild the transient
    ///      `included_fields_interned` caches.
    ///
    /// Unknown strings are silently skipped (they produce an empty inner vec
    /// for that path, which `build_covering_projection` will treat as absent).
    pub fn intern_included_paths(&self, interner: &Interner) {
        // Single COW pass: clone the current snapshot, mutate each def
        // in-place, store the new Arc. Replaces the previous per-key
        // DashMap::alter loop. Off hot path (called after register with
        // interner OR after load on bootstrap) so the one-shot Vec clone
        // is acceptable.
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<SortedIndexDefinition> = (*cur).clone();
            for def in new_vec.iter_mut() {
                if def.included_fields.is_empty() {
                    continue;
                }
                def.included_fields_interned = def
                    .included_fields
                    .iter()
                    .map(|path_segs| {
                        path_segs
                            .iter()
                            .filter_map(|seg| {
                                interner.touch_ind(seg.as_str()).ok().map(|t| t.key().id())
                            })
                            .collect::<Vec<u64>>()
                    })
                    .collect();
            }
            new_vec
        });
    }

    // ============================================================================
    // Planner methods — return Vec<IndexWriteOp> without side effects
    // ============================================================================

    /// Plan index entries for a newly created record.
    ///
    /// F-67 (#893): a PURE planner — no side effects, including no epoch
    /// bump. This is called at BOTH stage time (tx path, `version == 0`
    /// placeholder — see `table_manager_tx_ops.rs`) and apply time (non-tx
    /// direct path, via [`on_record_created`]'s real `version`), so bumping
    /// here would incorrectly fire at stage time for an uncommitted
    /// (possibly-aborting) tx. The per-index bump lives in
    /// [`on_record_created`], the actual apply-time entry point, derived
    /// from the ops this planner returns.
    pub fn plan_record_created(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::with_capacity(4);
        for def in &defs {
            if let Some(encoded) = extract_and_encode(record, &def.field_path)? {
                let key = self.build_entry_key(def.name_interned, &encoded, record_id);
                let value = if def.is_covering() {
                    build_covering_projection(record, def, version)
                } else {
                    Bytes::new()
                };
                ops.push(IndexWriteOp::SetPosting {
                    key,
                    value,
                    provenance: def.provenance(),
                });
            }
        }
        Ok(ops)
    }

    /// Planner variant of [`on_records_created_batch`] — collects
    /// entry ops for N records across all sorted indexes in one
    /// pass, snapshotting `iter_indexes()` ONCE (the per-row
    /// `plan_record_created` re-snapshots every call). Used by the
    /// tx batch insert path.
    ///
    /// F-67 (#893): a PURE planner — no epoch bump (see
    /// [`plan_record_created`]'s doc for why: this is also called at stage
    /// time with a `version == 0` placeholder).
    pub fn plan_records_created_batch<'a, R, I>(
        &self,
        items: I,
        version: u64,
    ) -> DbResult<Vec<IndexWriteOp>>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::new();
        for def in &defs {
            let provenance = def.provenance();
            for (rid, value) in items.clone() {
                if let Some(encoded) = extract_and_encode(value, &def.field_path)? {
                    let key = self.build_entry_key(def.name_interned, &encoded, rid);
                    let pv = if def.is_covering() {
                        build_covering_projection(value, def, version)
                    } else {
                        Bytes::new()
                    };
                    ops.push(IndexWriteOp::SetPosting {
                        key,
                        value: pv,
                        provenance,
                    });
                }
            }
        }
        Ok(ops)
    }

    /// Plan index entry changes when a record is updated.
    ///
    /// F-67 (#893): a PURE planner — no epoch bump (see
    /// [`plan_record_created`]'s doc for why: this is also called at stage
    /// time with a `version == 0` placeholder, e.g.
    /// `table_manager_tx_ops.rs`'s tx-stage path).
    pub fn plan_record_updated(
        &self,
        record_id: &RecordId,
        old: &(impl RecordRef + ?Sized),
        new: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::new();
        for def in &defs {
            let provenance = def.provenance();
            let old_enc = extract_and_encode(old, &def.field_path)?;
            let new_enc = extract_and_encode(new, &def.field_path)?;
            // For covering indexes, also rewrite the posting when the
            // projected values changed even if the indexed key did not.
            // Both old and new are built with the same `version` so that
            // the version bytes are identical and do not spuriously trigger
            // a rewrite when only the version changed.
            let old_proj = if def.is_covering() {
                Some(build_covering_projection(old, def, version))
            } else {
                None
            };
            let new_proj = if def.is_covering() {
                Some(build_covering_projection(new, def, version))
            } else {
                None
            };
            let key_changed = old_enc != new_enc;
            let proj_changed = old_proj != new_proj;
            if !key_changed && !proj_changed {
                continue;
            }
            if key_changed {
                if let Some(ref ov) = old_enc {
                    let key = self.build_entry_key(def.name_interned, ov, record_id);
                    ops.push(IndexWriteOp::RemovePosting {
                        key,
                        provenance,
                        owner: None,
                    });
                }
                if let Some(ref nv) = new_enc {
                    let key = self.build_entry_key(def.name_interned, nv, record_id);
                    let value = new_proj.clone().unwrap_or(Bytes::new());
                    ops.push(IndexWriteOp::SetPosting {
                        key,
                        value,
                        provenance,
                    });
                }
            } else {
                // Key is the same but projection changed — overwrite in place.
                if let Some(ref nv) = new_enc {
                    let key = self.build_entry_key(def.name_interned, nv, record_id);
                    let value = new_proj.clone().unwrap_or(Bytes::new());
                    ops.push(IndexWriteOp::SetPosting {
                        key,
                        value,
                        provenance,
                    });
                }
            }
        }
        Ok(ops)
    }

    /// Plan index entry removals for a deleted record.
    ///
    /// F-67 (#893): a PURE planner — no epoch bump (see
    /// [`plan_record_created`]'s doc for why: this is also called at stage
    /// time, e.g. `table_manager_tx_ops.rs::plan_base_index_delete_ops`).
    pub fn plan_record_deleted(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::new();
        for def in &defs {
            if let Some(encoded) = extract_and_encode(record, &def.field_path)? {
                let key = self.build_entry_key(def.name_interned, &encoded, record_id);
                ops.push(IndexWriteOp::RemovePosting {
                    key,
                    provenance: def.provenance(),
                    owner: None,
                });
            }
        }
        Ok(ops)
    }

    // ============================================================================
    // Apply ops
    // ============================================================================

    /// Apply a slice of `IndexWriteOp` against `self.info_store`.
    async fn apply_ops(&self, ops: &[IndexWriteOp]) -> DbResult<()> {
        for op in ops {
            match op {
                IndexWriteOp::SetPosting { key, value, .. } => {
                    self.info_store
                        .set(key.clone().into(), value.clone())
                        .await?;
                }
                IndexWriteOp::RemovePosting { key, .. } => {
                    let _ = self.info_store.remove(key.clone().into()).await?;
                }
                IndexWriteOp::BumpFtsStats { .. } => {
                    // Not relevant for SortedIndexManager.
                }
            }
        }
        Ok(())
    }

    /// F-67 (#893): bump the per-index mutation high-water to `version` for
    /// every DISTINCT sorted-index `name_interned` touched by `ops`.
    ///
    /// Called at APPLY time only:
    /// - Internally, right after a planner's pure `Vec<IndexWriteOp>` is
    ///   produced by one of the `on_record_*` wrappers below (the non-tx
    ///   direct path's own apply point) — never inside the planners
    ///   themselves, which are also invoked at STAGE time (tx path,
    ///   `version == 0` placeholder) where bumping would incorrectly disable
    ///   the fast path for an uncommitted, possibly-aborting tx.
    /// - Externally, by `commit_phases.rs::apply_index_batch` (the tx-commit
    ///   path's Phase 5c apply point) against the flat `ops: &[IndexWriteOp]`
    ///   batch for one table. F-74 (#901): called BEFORE the postings land
    ///   (mirroring the non-tx path above), not after — `tx.
    ///   index_write_set` is `Vec<(table_token, IndexWriteOp)>`, grouped only
    ///   by table (never by index), so this is the only per-index-precise
    ///   entry point available at that call site.
    ///
    /// `ops` may legitimately contain entries this manager did NOT produce
    /// (other index families — hash/FTS/functional — share the same flat
    /// `IndexWriteOp` enum and the same commit-time batch; `IndexWriteOp`
    /// carries no index-id field of its own). Each key is decoded via
    /// [`decode_sorted_index_name`] (the `[SORTED_TAG | name_interned |
    /// ...]` layout [`Self::build_entry_key`] writes); a `None` (foreign key
    /// from a different index family, or a malformed key) is skipped rather
    /// than treated as an error. F-74 (#901): a decode miss on a key that
    /// genuinely WAS a sorted-index posting is NOT harmless — the gate this
    /// counter drives is `epoch <= pinned ⟹ take the fast path`, so a missed
    /// bump leaves the epoch LOW and the gate OPEN, i.e. it KEEPS the fast
    /// path enabled for an index whose postings just changed, which is the
    /// unsafe direction (a false-negative here risks a wrong AsOf page, not
    /// a spurious full-scan fallback). Before F-67 (#893) the bump was
    /// UNCONDITIONAL, so the only possible error was OVER-bumping (safe:
    /// closes the gate, forces a fallback); F-67 made the bump conditional on
    /// `decode_sorted_index_name` returning `Some`, introducing the
    /// possibility of UNDER-bumping this doc previously mis-described as
    /// harmless. `decode_sorted_index_name`'s own decode correctness is out
    /// of scope here — this note only corrects the description of what a
    /// miss actually costs.
    /// De-duplicates via a small on-stack scan (`SmallVec`) since a table
    /// typically has ≤ ~10 sorted indexes (see the manager's cardinality
    /// doc) — cheaper than allocating a `HashSet` for single-digit N.
    pub fn bump_touched_indexes(&self, ops: &[IndexWriteOp], version: u64) {
        let mut touched: SmallVec<[u64; 8]> = SmallVec::new();
        for op in ops {
            let key = match op {
                IndexWriteOp::SetPosting { key, .. } => key,
                IndexWriteOp::RemovePosting { key, .. } => key,
                IndexWriteOp::BumpFtsStats { .. } => continue,
            };
            if let Some(name_interned) = decode_sorted_index_name(key.as_ref()) {
                if !touched.contains(&name_interned) {
                    touched.push(name_interned);
                    self.note_mutation_at_version(name_interned, version);
                }
            }
        }
    }

    // ============================================================================
    // on_record_* wrappers — plan + apply
    // ============================================================================

    /// Add an index entry for a record. Called from
    /// `TableManager::insert` and `set` (create branch).
    pub async fn on_record_created(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<()> {
        let ops = self.plan_record_created(record_id, record, version)?;
        // F-67 (#893): bump at APPLY time (this method is the non-tx direct
        // path's apply point), only for the index(es) `ops` actually touched.
        self.bump_touched_indexes(&ops, version);
        self.apply_ops(&ops).await
    }

    /// Update entries when a record changes.
    pub async fn on_record_updated(
        &self,
        record_id: &RecordId,
        old: &(impl RecordRef + ?Sized),
        new: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<()> {
        let ops = self.plan_record_updated(record_id, old, new, version)?;
        // F-67 (#893): bump at APPLY time (see on_record_created).
        self.bump_touched_indexes(&ops, version);
        self.apply_ops(&ops).await
    }

    /// Batched version of `on_record_created` — collects all entry
    /// writes across all sorted indexes for N records into one
    /// `Store::set_many` call. Borrow-only — no `InnerValue` clones
    /// except for covering-index projection (unavoidable: projection
    /// requires a deep clone of the leaf value).
    pub async fn on_records_created_batch<'a, R, I>(&self, items: I, version: u64) -> DbResult<()>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        if self.indexes.load_local().is_empty() {
            return Ok(());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        // `RecordKey` keys (fed to the store `set_many`); entry keys are
        // built as `Bytes` and converted byte-identically at each push.
        let mut writes: Vec<(RecordKey, Bytes)> = Vec::new();
        for def in &defs {
            let mut touched = false;
            for (rid, value) in items.clone() {
                if let Some(encoded) = extract_and_encode(value, &def.field_path)? {
                    let key = self.build_entry_key(def.name_interned, &encoded, rid);
                    let pv = if def.is_covering() {
                        build_covering_projection(value, def, version)
                    } else {
                        Bytes::new()
                    };
                    writes.push((key.into(), pv));
                    touched = true;
                }
            }
            // F-67 (#893): bump at APPLY time (this method is the non-tx
            // direct batch path's apply point) — only for a `def` at least
            // one item in the batch actually produced a posting for.
            if touched {
                self.note_mutation_at_version(def.name_interned, version);
            }
        }
        if writes.is_empty() {
            return Ok(());
        }
        self.info_store.set_many(writes).await?;
        Ok(())
    }

    /// Drop entries for a deleted record.
    ///
    /// `version` is the DELETE's MVCC commit version, used to bump the
    /// per-index mutation high-water at APPLY time (F-67, #893: only for the
    /// index(es) that actually carried a posting for this record — see
    /// [`Self::bump_touched_indexes`]). A DELETE after a cursor's pin
    /// REMOVES the posting for that index — the index never yields the
    /// record id again, so a seek planned against THAT SAME index would
    /// silently miss a row the pinned snapshot must show. The gate (bumped
    /// here) is what makes the AsOf seek decline in that case and fall back
    /// to the full scan; mutating an UNRELATED index no longer bumps this
    /// counter at all.
    pub async fn on_record_deleted(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<()> {
        let ops = self.plan_record_deleted(record_id, record)?;
        // F-67 (#893): bump at APPLY time (see on_record_created).
        self.bump_touched_indexes(&ops, version);
        self.apply_ops(&ops).await
    }
}
