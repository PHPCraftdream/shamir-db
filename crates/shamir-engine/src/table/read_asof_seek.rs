//! F-53b Step 2 (#878): AsOf-aware sorted-index keyset seek for `read_as_of`.
//!
//! Sibling of `read_index_scan.rs::read_keyset_seek` (the `Temporal::Latest`
//! fast path). Mirrors its ordered-walk + stale-posting-resume loop, but adds
//! the per-candidate MVCC-version classifier proven by F-53b Step 1's spike
//! (`docs/dev-artifacts/research/f53b-cursor-seek-spike.md`): each index
//! candidate is classified against the cursor's pinned snapshot version via
//! `MvccStore::version_of` + `get_at`, so a row INSERTed after the pin is
//! correctly excluded (snapshot isolation preserved) while the seek cost stays
//! `O(page_size)` per page instead of the `O(N)` the full-rescan
//! `read_as_of` baseline pays.
//!
//! The §5.1 gate (`SortedIndexManager::last_mutation_version() <= pinned`) is
//! checked by the caller (`read_as_of`) BEFORE dispatching here. It guarantees
//! that, AT ENTRY, the current-state index provably mirrors the pinned
//! snapshot's postings, so the §1.3 cases (UPDATE to the indexed field,
//! DELETE after pin) — which a current-state index CANNOT serve — could not
//! already have happened. The `concurrent_modified` counter classifies any
//! candidate the walk actually OBSERVES with a bumped version — but it cannot
//! catch a posting that a concurrent mutation REMOVES from the observed range
//! entirely (DELETE) or MOVES out of it (an UPDATE to the indexed field): such
//! a posting is simply absent from the walk, never reaches the classifier,
//! and the page would silently omit a row that existed in the pinned
//! snapshot.
//!
//! F-58 (#884) closes this: the entry gate is only checked ONCE, but the walk
//! below is a multi-iteration, multi-`.await` loop — a mutation landing
//! DURING that window is a genuine TOCTOU race the entry check alone cannot
//! catch. The fix is a symmetric POST-scan re-check of the same predicate
//! (`last_mutation_version() > pinned_version` ⇒ fall back), added after the
//! loop and the `concurrent_modified` check, before this arm commits to
//! returning its result. This works because `last_mutation_version` is a
//! monotonic, `Acquire`/`AcqRel`-ordered high-water mark
//! (`shamir-index/src/legacy/sorted_index_manager.rs`) — any mutation whose
//! effect the walk could have observed necessarily bumps this same counter,
//! and the post-check's `Acquire` load is guaranteed to see it (or later).
//!
//! ## Bump-vs-apply ordering (both verified, both safe for the post-check)
//!
//! - **Non-tx direct path** (`on_record_created`/`on_record_updated`/
//!   `on_records_created_batch`/`on_record_deleted`,
//!   `sorted_index_manager.rs:573-657`): bumps the high-water FIRST, applies
//!   the posting mutation SECOND. This is actually the SAFER order for the
//!   post-check: if the scan ever observes the mutation's effect, the bump
//!   necessarily already happened — zero residual window. The only cost is a
//!   possible false-positive fallback (bump fired, apply hadn't landed yet),
//!   which is always safe (the full-scan fallback is correct by
//!   construction).
//! - **Tx-commit path** (`commit_phases.rs` Phase 5c): applies the posting
//!   mutation FIRST, bumps the high-water SECOND — the ordering this module
//!   originally assumed. This leaves a narrow, synchronous (no `.await`
//!   between the two calls) window where the post-check could in principle
//!   run between the apply and the bump and miss it. This residual is
//!   accepted as out-of-scope for F-58: it is orders of magnitude narrower
//!   than the unbounded-scan-duration TOCTOU this task closes, and closing it
//!   fully would mean reordering the tx-commit pipeline's Phase 5c itself —
//!   separate, riskier work, not this read-path fix.

use std::time::Instant;

use bytes::Bytes;
use shamir_query_types::read::OrderDirection;
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{exec, QueryRecord, QueryResult, QueryStats, ReadQuery};
use shamir_storage::error::DbResult;

use super::read_exec::apply_select_value_bytes;
use super::table_manager::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// F-58 (#884): test-only pause seam — parks `read_as_of_keyset_seek` AFTER
// the entry gate has passed (the caller checks `last_mutation_version() <=
// pinned` before dispatching here) but BEFORE the loop's first iteration.
//
// Mirrors the `PostBarrierPreWriteHook` / `TEST_POST_BARRIER_PRE_WRITE_HOOK`
// convention (table_manager_crud.rs, F-48 #859). One-shot: only the FIRST
// seek to reach this seam actually parks (CAS true→false); every later seek
// passes straight through. `nextest` runs each test in its own process, so
// the global cannot leak across test files.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct SeekLoopPreIterHook {
    pub(crate) reached: std::sync::atomic::AtomicUsize,
    pub(crate) resume: tokio::sync::Notify,
    pub(crate) armed: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
pub(crate) static TEST_SEEK_LOOP_PRE_ITER_HOOK: std::sync::OnceLock<
    std::sync::Arc<SeekLoopPreIterHook>,
> = std::sync::OnceLock::new();

/// Parks on [`TEST_SEEK_LOOP_PRE_ITER_HOOK`] if a test installed one;
/// a true no-op otherwise.
async fn fire_seek_loop_pre_iter_test_hook() {
    #[cfg(test)]
    if let Some(hook) = TEST_SEEK_LOOP_PRE_ITER_HOOK.get() {
        hook.reached
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let should_pause = hook
            .armed
            .compare_exchange(
                true,
                false,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok();
        if should_pause {
            hook.resume.notified().await;
        }
    }
}

impl TableManager {
    /// F-53b Step 2 (#878): AsOf-aware sorted-index keyset seek.
    ///
    /// Walks the sorted index in ORDER BY direction via
    /// `lookup_range_first_k_page` (the SAME primitive the
    /// `Temporal::Latest` keyset-seek fast path uses), and for EACH candidate
    /// record id applies a pinned-version visibility check against the MVCC
    /// store — the classifier proven by F-53b Step 1:
    ///
    /// - `version_of(id) != 0 && <= pinned` ⇒ the record's CURRENT committed
    ///   value was written at/before the pin, so (MVCC immutability) its AS-OF
    ///   value EQUALS its current value and the index posting's position is
    ///   correct for the pinned snapshot. Read it via `get_at(id, pinned)` and
    ///   include it.
    /// - `version_of(id) > pinned || == 0` + `get_at(id, pinned) == None` ⇒
    ///   INSERT-after-pin: the record did not exist at the pin. EXCLUDE.
    /// - `version_of(id) > pinned || == 0` + `get_at(id, pinned) == Some` ⇒
    ///   MODIFY-after-pin (UPDATE to the indexed field or a re-creation after
    ///   a post-pin delete). Flag `concurrent_modified`; under a correct
    ///   §5.1 gate this case is impossible. If it fires, this method returns
    ///   `Ok(None)` so the caller falls back to the full scan.
    ///
    /// Returns `Ok(Some(result))` on a successful page. Returns `Ok(None)` as
    /// the defence-in-depth fallback signal when `concurrent_modified > 0` is
    /// detected — the caller (`read_as_of`) then falls through to the existing
    /// full-scan path for this page.
    ///
    /// `records_scanned` in the returned stats is the number of physical index
    /// postings the seek walked (`candidates_examined` in the spike), NOT the
    /// full table size — this is the asymptotic win: `O(page_size)` here vs
    /// `O(N)` for the baseline.
    #[allow(clippy::too_many_arguments)] // read-path parameters mirror read_keyset_seek
    pub(super) async fn read_as_of_keyset_seek(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        index_name: u64,
        encoded_key: &[u8],
        after_id: Option<&RecordId>,
        limit: usize,
        direction: OrderDirection,
        pinned_version: u64,
        start: Instant,
    ) -> DbResult<Option<QueryResult>> {
        let mvcc = self.mvcc_store_ref().ok_or_else(|| {
            shamir_storage::error::DbError::Validation(
                "AsOf keyset seek requires an MVCC-backed table".to_string(),
            )
        })?;

        let forward = matches!(direction, OrderDirection::Asc);

        // F-58 (#884): test seam — park AFTER the entry gate passed (the
        // caller already checked `last_mutation_version() <= pinned`) but
        // BEFORE the loop starts reading postings. This is the TOCTOU window
        // the post-scan re-check below closes.
        fire_seek_loop_pre_iter_test_hook().await;

        // Ordered walk + stale-posting-resume loop — mirrors
        // `read_keyset_seek`'s loop, generalised to also resume past
        // version-filtered candidates (INSERT-after-pin exclusions).
        let mut matched: Vec<(RecordId, Bytes)> = Vec::with_capacity(limit);
        let mut after_key: Option<Bytes> = None;
        let mut candidates_examined: u64 = 0;
        let mut concurrent_modified: u64 = 0;

        loop {
            let need = limit - matched.len();
            // `after_id` is the fixed client reference (the row the CLIENT
            // already has from a PREVIOUS request). It only affects rows whose
            // encoded value still equals `encoded_key`, so re-passing it on
            // every internal-loop iteration is safe (mirrors `read_keyset_seek`
            // / task #537's fix).
            let (id_vec, cursor) = self
                .sorted_indexes()
                .lookup_range_first_k_page(
                    index_name,
                    encoded_key,
                    after_key.as_deref(),
                    after_id,
                    need,
                    forward,
                )
                .await?;
            candidates_examined += id_vec.len() as u64;

            // CR-C3 (#778): batch-fetch the AS-OF bytes for all candidates in
            // one vectored `get_at_many` call instead of one `get_at` await
            // per record.
            let keys: Vec<Bytes> = id_vec.iter().map(|id| id.to_bytes()).collect();
            let asof_values = mvcc.get_at_many(&keys, pinned_version).await?;

            for (id, asof_bytes) in id_vec.iter().zip(asof_values) {
                let v = mvcc.version_of(id.as_bytes());
                if v != 0 && v <= pinned_version {
                    // Common path: no concurrent write on this key since the
                    // pin. By MVCC immutability the current value IS the
                    // pinned value, so the posting's position is correct for
                    // the snapshot.
                    if let Some(bytes) = asof_bytes {
                        matched.push((*id, bytes));
                        if matched.len() == limit {
                            break;
                        }
                    }
                    // A `None` here is a tombstone/no-value edge — skip.
                } else {
                    // A write landed on this key after the pin (or the cell is
                    // cold). Distinguish INSERT-after-pin from MODIFY-after-pin.
                    match asof_bytes {
                        None => {} // INSERT-after-pin: correctly excluded.
                        Some(_) => {
                            // MODIFY-after-pin: the posting's current position
                            // is NOT trustworthy for the pinned snapshot. Under
                            // a correct §5.1 gate this is impossible; keep as
                            // defence-in-depth and signal fallback below.
                            concurrent_modified += 1;
                        }
                    }
                }
            }

            // Stop when the page filled `limit`, or the index range is truly
            // exhausted (no resume cursor) — a genuine short last page.
            match cursor {
                Some(c) if matched.len() < limit => after_key = Some(c),
                _ => break,
            }
        }

        // Defence-in-depth: if the §5.1 gate somehow missed (should be
        // impossible — the caller checks `last_mutation_version() <= pinned`
        // before dispatching here), fall back to the full scan for this page
        // rather than returning a wrong result.
        if concurrent_modified > 0 {
            return Ok(None);
        }

        // F-58 (#884): post-scan re-check of the entry gate's predicate. The
        // entry gate (checked once in `read_as_of` before dispatching here)
        // can be invalidated by a concurrent UPDATE/DELETE landing DURING the
        // loop above — a posting the walk has not yet reached (or already
        // passed) can vanish entirely, and the `concurrent_modified` classifier
        // above can only flag candidates it actually observes. This symmetric
        // re-check closes that TOCTOU window: any mutation that could have
        // raced the walk bumps this same counter, and the post-check's Acquire
        // load observes it (see the module doc for the bump-vs-apply ordering
        // analysis). If this fires, the caller falls through to the full scan
        // — correct by construction.
        if self.sorted_indexes().last_mutation_version() > pinned_version {
            return Ok(None);
        }

        // Project to QueryValue in value order — no sort, no exclusion filter
        // (both are handled by the ordered early-stop walk + the version
        // classifier above). Mirrors `read_keyset_seek`'s tail.
        let mut corrupt: Vec<crate::query::read::CorruptRecordRef> = Vec::new();
        let result_qv = apply_select_value_bytes(
            &matched,
            &query.select,
            interner,
            ctx.scalars.clone(),
            self.name(),
            &mut corrupt,
        )?;

        let records_scanned = candidates_examined;
        let records_returned = result_qv.len() as u64;

        // Task #537: surface each row's stable identity so a client can echo
        // it back as the `after_id` tie-breaker on the next page request
        // (same wire pattern as `read_keyset_seek`). This is additive: the
        // `_id` key appears in the row map alongside the projected fields;
        // clients that ignore it are unaffected.
        let records: Vec<QueryRecord> = matched
            .iter()
            .map(|(id, _)| *id)
            .zip(result_qv)
            .map(|(id, fields)| {
                QueryRecord::Inserted(shamir_query_types::write::InsertedRecord {
                    id: Some(id),
                    fields,
                })
            })
            .collect();

        Ok(Some(QueryResult {
            records,
            stats: Some(QueryStats {
                index_used: Some(format!("sorted_idx_{index_name}_asof_keyset")),
                records_scanned,
                records_returned,
                execution_time_us: start.elapsed().as_micros() as u64,
            }),
            pagination: exec::fast_path_pagination(&query.pagination),
            value: None,
            explain: None,
            skipped: false,
            versions: None,
            corrupt_records: corrupt,
        }))
    }
}
