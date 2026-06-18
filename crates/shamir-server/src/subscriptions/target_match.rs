use shamir_collections::TFxMap;
use shamir_query_types::filter::Filter;
use shamir_query_types::subscribe::event_mask::EventMask;
use shamir_tx::ChangeOp;

use super::decode_cache::CachedRecordBytes;
use super::filter_eval::filter_matches_bytes;

/// Per-bridge index built once at subscribe time:
/// maps `(repo_idx, table_name)` -> sorted list of target indices
/// that care about that exact (repo, table) pair.
///
/// This collapses the two O(T) linear scans in `any_target_interested` /
/// `matches_any` to a single O(1) HashMap lookup followed by an O(k) walk
/// over only the k <= T relevant targets.
pub type TargetIndex = TFxMap<(usize, String), Vec<usize>>;

/// Build the per-bridge target index from the targets vec and the
/// `repo_idx` map (repo name -> position in `repos`).
pub fn build_target_index(
    targets: &[(String, String, EventMask, Option<Filter>)],
    repo_idx: &TFxMap<String, usize>,
) -> TargetIndex {
    let mut index: TargetIndex = TFxMap::default();
    for (i, (repo, table, _mask, _filter)) in targets.iter().enumerate() {
        if let Some(&ri) = repo_idx.get(repo.as_str()) {
            index.entry((ri, table.clone())).or_default().push(i);
        }
    }
    index
}

/// O(1) gate: are any targets interested in `(repo_idx, table, op)`?
/// Returns the slice of target indices to iterate, or an empty slice.
#[inline]
pub fn indexed_targets<'a>(
    index: &'a TargetIndex,
    repo_idx: usize,
    table: &str,
) -> Option<&'a [usize]> {
    // Temporary owned key for lookup -- avoids a HashMap<(usize, &str), ...> which
    // would require a custom Borrow impl. The table String is typically short
    // (<= 64 bytes) so the allocation is cheap relative to hash probe savings.
    index.get(&(repo_idx, table.to_owned())).map(Vec::as_slice)
}

/// O(1)-gated pre-check using the target index.
#[inline]
pub fn any_target_interested_indexed(
    targets: &[(String, String, EventMask, Option<Filter>)],
    index: &TargetIndex,
    repo_idx: usize,
    table: &str,
    op: &ChangeOp,
) -> bool {
    match indexed_targets(index, repo_idx, table) {
        None => false,
        Some(idxs) => idxs.iter().any(|&i| mask_matches(&targets[i].2, op)),
    }
}

/// O(1)-gated full filter match using the target index.
///
/// `bytes_decoded` carries the raw msgpack record bytes and the table's
/// `Arc<OnceCell<Interner>>` (guaranteed populated) needed to construct a
/// zero-copy `RecordView` lens for filter evaluation.
/// Value decoding is not performed here -- that is deferred to the deliver path.
pub fn matches_any_indexed(
    targets: &[(String, String, EventMask, Option<Filter>)],
    index: &TargetIndex,
    repo_idx: usize,
    table: &str,
    op: &ChangeOp,
    bytes_decoded: Option<&CachedRecordBytes>,
) -> bool {
    match indexed_targets(index, repo_idx, table) {
        None => false,
        Some(idxs) => idxs.iter().any(|&i| {
            let (_, _, mask, filter) = &targets[i];
            if !mask_matches(mask, op) {
                return false;
            }
            match (filter, op) {
                (Some(f), ChangeOp::Put) => match bytes_decoded {
                    Some((bytes, interner_cell)) => filter_matches_bytes(f, bytes, interner_cell),
                    None => {
                        tracing::warn!(
                            "subscription filter: no bytes for Put value, \
                             skipping event (fail-closed)"
                        );
                        false
                    }
                },
                _ => true,
            }
        }),
    }
}

/// Cheap synchronous gate: does any target want this (repo, table, op) at all,
/// ignoring filter evaluation? Used to short-circuit the per-change async
/// decode call when no subscriber could possibly match.
/// Filter evaluation still runs in `matches_any` for surviving changes.
#[inline]
pub fn any_target_interested(
    targets: &[(String, String, EventMask, Option<Filter>)],
    repo: &str,
    table: &str,
    op: &ChangeOp,
) -> bool {
    targets
        .iter()
        .any(|(target_repo, target_table, mask, _filter)| {
            target_repo == repo && target_table == table && mask_matches(mask, op)
        })
}

pub fn matches_any(
    targets: &[(String, String, EventMask, Option<Filter>)],
    repo: &str,
    table: &str,
    op: &ChangeOp,
    bytes_decoded: Option<&CachedRecordBytes>,
) -> bool {
    targets
        .iter()
        .any(|(target_repo, target_table, mask, filter)| {
            if target_repo != repo || target_table != table || !mask_matches(mask, op) {
                return false;
            }
            match (filter, op) {
                (Some(f), ChangeOp::Put) => match bytes_decoded {
                    Some((bytes, interner_cell)) => filter_matches_bytes(f, bytes, interner_cell),
                    None => {
                        tracing::warn!(
                            "subscription filter: no bytes for Put value, \
                             skipping event (fail-closed)"
                        );
                        false
                    }
                },
                _ => true,
            }
        })
}

#[inline]
pub fn mask_matches(mask: &EventMask, op: &ChangeOp) -> bool {
    matches!(
        (mask, op),
        (EventMask::All, _)
            | (EventMask::Put, ChangeOp::Put)
            | (EventMask::Delete, ChangeOp::Delete)
    )
}
