//! FG-3: tx-scan read-your-own-writes — merge a tx's staged `write_set`
//! overlay on top of a committed-store record stream.
//!
//! Mirrors `shamir_storage::storage_membuffer::merge_overlay_stream`'s
//! algorithm exactly (task #530): a classic 2-way sorted merge between a
//! sorted overlay snapshot and a sorted `inner` stream. `inner` MUST yield
//! `RecordId`-ascending batches (every committed-store scan in this crate
//! upholds that). The overlay is pre-sorted by `RecordId` by the caller
//! ([`overlay_rows_for_tx`]).
//!
//! Semantics:
//! * A staged `Set` ALWAYS wins over `inner` for the same key (it is the
//!   newer, not-yet-committed value) — emitted as `RecordCow::Borrowed`
//!   (zero-copy, no decode).
//! * A staged `Removed` masks the key — excluded even if `inner` still
//!   yields a stale committed value for it.
//! * Overlay-only keys (staged inserts never yet in the committed store)
//!   are interleaved in sorted position, including a tail flush for
//!   overlay keys sorting after the last `inner` key.
//! * Ordering across the merged output is preserved (ascending).
//!
//! Two entry points cover Part 2 of the brief:
//! - [`merge_stream_with_tx_overlay`] — unfiltered pass-through
//!   (`list_stream_tx`).
//! - [`merge_filtered_stream_with_tx_overlay`] — every overlay-sourced row
//!   (staged insert OR staged-overridden update) is re-evaluated against
//!   the caller's compiled filter before being yielded; a staged row that
//!   does not match is EXCLUDED (`filter_stream_tx`, and the match-scans in
//!   `write_exec.rs`).

use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_types::types::record_id::RecordId;

use super::record_cow::RecordCow;

/// A staged row from a tx's `write_set` overlay, pre-classified.
pub(super) enum StagedRowOverlay {
    /// The tx staged a `Set` — carries the staged storage bytes.
    Set(Bytes),
    /// The tx staged a `Remove` — the key is hidden from the merged output.
    Removed,
}

/// Build the sorted `(RecordId, StagedRowOverlay)` overlay for `token` from
/// `tx.write_set`. Returns an empty vec when the tx never wrote this table
/// (`tx.write_set.get(token) == None`) — the zero-cost pass-through case the
/// brief mandates (mirrors `record_scan_reads`'s "gate before any work").
///
/// A staged key that fails to parse back into a 16-byte `RecordId` is
/// skipped defensively (data-table staging keys are always `RecordId`
/// bytes in every production write path; a malformed key here would
/// indicate a bug elsewhere, not something this merge should propagate as
/// an error).
pub(super) fn overlay_rows_for_tx(
    tx: Option<&shamir_tx::TxContext>,
    token: u64,
) -> Vec<(RecordId, StagedRowOverlay)> {
    let Some(tx) = tx else {
        return Vec::new();
    };
    let Some(staging) = tx.write_set.get(&token) else {
        return Vec::new();
    };
    let mut rows: Vec<(RecordId, StagedRowOverlay)> = staging
        .snapshot_ops()
        .into_iter()
        .filter_map(|op| match op {
            shamir_storage::types::KvOp::Set(k, v) => {
                RecordId::try_from_bytes(k.as_slice()).map(|id| (id, StagedRowOverlay::Set(v)))
            }
            shamir_storage::types::KvOp::Remove(k) => {
                RecordId::try_from_bytes(k.as_slice()).map(|id| (id, StagedRowOverlay::Removed))
            }
        })
        .collect();
    rows.sort_by_key(|(id, _)| *id);
    rows
}

/// Merge `inner` (a committed-store record stream, `RecordId`-ascending)
/// with `overlay` (this tx's staged ops for the table, pre-sorted by
/// `RecordId` — see [`overlay_rows_for_tx`]) so a staged `Set`
/// overrides/injects, a staged `Remove` hides, and rows with no staged op
/// pass through unchanged. Unfiltered variant — used by `list_stream_tx`,
/// which has no `Filter`/`FilterContext` to re-evaluate overlay rows
/// against.
pub(super) fn merge_stream_with_tx_overlay<'a>(
    inner: impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a,
    overlay: Vec<(RecordId, StagedRowOverlay)>,
    batch_size: usize,
) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a {
    merge_impl(inner, overlay, batch_size, |_id, bytes| Some(bytes))
}

/// Filtered variant of [`merge_stream_with_tx_overlay`]: every overlay-
/// sourced row (staged insert OR staged-overridden update) is passed
/// through `keep` (a compiled-filter match closure) before being yielded.
/// `keep` returns `true` to include the row, `false` to exclude it — a
/// staged UPDATE's new value may match/not-match the filter differently
/// than the old committed value did, and a staged INSERT was never
/// filtered at all since it never went through a scan before. Rows sourced
/// from `inner` (no staged op) are NOT re-filtered here — `inner` is
/// assumed to already be a filtered stream (e.g. `filter_stream`'s output).
pub(super) fn merge_filtered_stream_with_tx_overlay<'a, F>(
    inner: impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a,
    overlay: Vec<(RecordId, StagedRowOverlay)>,
    batch_size: usize,
    keep: F,
) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a
where
    F: Fn(&RecordId, &Bytes) -> bool + 'a,
{
    merge_impl(inner, overlay, batch_size, move |id, bytes| {
        if keep(id, &bytes) {
            Some(bytes)
        } else {
            None
        }
    })
}

/// Find every purely-staged-inserted row (a staged `Set` whose key is NOT
/// already in `already_matched`) that satisfies `keep`, for use by
/// `execute_update_tx` / `execute_delete_tx`'s match-scans (Part 2 items
/// 3-5 of the FG-3 brief).
///
/// This does NOT re-emit rows `already_matched` already found (those are
/// handled by each caller's existing per-row staging-first merge — see
/// `execute_update_tx`'s `effective_old_bytes` resolution) — it only
/// closes the gap where a row this tx staged (insert OR update-with-new-
/// bytes) never entered `already_matched` in the first place because the
/// committed-store scan is blind to `tx.write_set`. A staged `Remove` is
/// never returned (nothing to match — the row is gone from this tx's point
/// of view).
///
/// `keep(id, bytes)` is the caller's compiled-filter match test (or the
/// unconditional-match closure `|_, _| true` for a DELETE/UPDATE with no
/// WHERE clause).
pub(super) fn staged_only_matches<F>(
    tx: Option<&shamir_tx::TxContext>,
    token: u64,
    already_matched: &shamir_collections::TSet<RecordId>,
    keep: F,
) -> Vec<(RecordId, Bytes)>
where
    F: Fn(&RecordId, &Bytes) -> bool,
{
    let Some(tx) = tx else {
        return Vec::new();
    };
    let Some(staging) = tx.write_set.get(&token) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for op in staging.snapshot_ops() {
        if let shamir_storage::types::KvOp::Set(k, v) = op {
            let Some(id) = RecordId::try_from_bytes(k.as_slice()) else {
                continue;
            };
            if already_matched.contains(&id) {
                continue;
            }
            if keep(&id, &v) {
                out.push((id, v));
            }
        }
    }
    out
}

/// Shared merge engine for both variants above. `admit` decides whether an
/// overlay-sourced `Set` row's bytes are actually yielded (unfiltered
/// variant always admits; filtered variant re-checks the caller's filter).
/// A staged `Remove` is never admitted (tombstone), regardless of `admit`.
fn merge_impl<'a, A>(
    inner: impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a,
    overlay: Vec<(RecordId, StagedRowOverlay)>,
    batch_size: usize,
    admit: A,
) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a
where
    A: Fn(&RecordId, Bytes) -> Option<Bytes> + 'a,
{
    async_stream::stream! {
        // Zero-cost pass-through when this tx never wrote the table.
        if overlay.is_empty() {
            futures::pin_mut!(inner);
            while let Some(batch) = futures::StreamExt::next(&mut inner).await {
                yield batch;
            }
            return;
        }

        let batch_cap = batch_size.max(1);
        let mut ov = overlay.into_iter().peekable();
        let mut out: Vec<(RecordId, RecordCow)> = Vec::with_capacity(batch_cap);

        futures::pin_mut!(inner);
        while let Some(batch_result) = futures::StreamExt::next(&mut inner).await {
            let batch = match batch_result {
                Ok(b) => b,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };
            for (ik, iv) in batch {
                // Emit every overlay entry that sorts strictly before the
                // current inner key (overlay-only keys interleaved in order).
                while let Some((ok, _)) = ov.peek() {
                    if *ok < ik {
                        let (ok, row) = ov.next().unwrap();
                        if let StagedRowOverlay::Set(bytes) = row {
                            if let Some(bytes) = admit(&ok, bytes) {
                                out.push((ok, RecordCow::Borrowed(bytes)));
                                if out.len() >= batch_cap {
                                    yield Ok(std::mem::take(&mut out));
                                    out = Vec::with_capacity(batch_cap);
                                }
                            }
                        }
                        // Removed-only overlay key: nothing to emit.
                    } else {
                        break;
                    }
                }
                // Overlay entry for the EXACT inner key wins (newer write).
                if let Some((ok, _)) = ov.peek() {
                    if *ok == ik {
                        let (ok, row) = ov.next().unwrap();
                        match row {
                            StagedRowOverlay::Set(bytes) => {
                                if let Some(bytes) = admit(&ok, bytes) {
                                    out.push((ok, RecordCow::Borrowed(bytes)));
                                    if out.len() >= batch_cap {
                                        yield Ok(std::mem::take(&mut out));
                                        out = Vec::with_capacity(batch_cap);
                                    }
                                }
                            }
                            // Tombstone masks the stale inner value → skip.
                            StagedRowOverlay::Removed => {}
                        }
                        continue;
                    }
                }
                // No overlay entry for this key → inner value stands
                // unchanged (already filtered/decoded by the caller).
                out.push((ik, iv));
                if out.len() >= batch_cap {
                    yield Ok(std::mem::take(&mut out));
                    out = Vec::with_capacity(batch_cap);
                }
            }
        }

        // Overlay-only tail: any remaining overlay entries (keys `inner`
        // never yielded — purely-staged inserts sorting after the last
        // committed key). Sets are emitted (subject to `admit`); removes
        // are dropped (there is nothing to tombstone in a stream `inner`
        // never yielded anyway).
        for (ok, row) in ov {
            if let StagedRowOverlay::Set(bytes) = row {
                if let Some(bytes) = admit(&ok, bytes) {
                    out.push((ok, RecordCow::Borrowed(bytes)));
                    if out.len() >= batch_cap {
                        yield Ok(std::mem::take(&mut out));
                        out = Vec::with_capacity(batch_cap);
                    }
                }
            }
        }

        if !out.is_empty() {
            yield Ok(out);
        }
    }
}
