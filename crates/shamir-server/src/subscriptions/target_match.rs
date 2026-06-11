use shamir_query_types::filter::Filter;
use shamir_query_types::subscribe::event_mask::EventMask;
use shamir_tx::ChangeOp;

use super::filter_eval::filter_matches_value;

/// Cheap synchronous gate: does any target want this (repo, table, op) at all,
/// ignoring filter evaluation? Used to short-circuit the per-change async
/// `decode_record_value_json` call when no subscriber could possibly match.
/// Filter evaluation still runs in `matches_any` for surviving changes.
pub(crate) fn any_target_interested(
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

pub(crate) fn matches_any(
    targets: &[(String, String, EventMask, Option<Filter>)],
    repo: &str,
    table: &str,
    op: &ChangeOp,
    value: Option<&serde_json::Value>,
) -> bool {
    targets
        .iter()
        .any(|(target_repo, target_table, mask, filter)| {
            if target_repo != repo || target_table != table || !mask_matches(mask, op) {
                return false;
            }
            match (filter, op) {
                (Some(f), ChangeOp::Put) => match value {
                    Some(v) => filter_matches_value(f, v),
                    None => {
                        tracing::warn!(
                            "subscription filter: de-intern decode failed for Put value, \
                             skipping event (fail-closed)"
                        );
                        false
                    }
                },
                _ => true,
            }
        })
}

pub(crate) fn mask_matches(mask: &EventMask, op: &ChangeOp) -> bool {
    matches!(
        (mask, op),
        (EventMask::All, _)
            | (EventMask::Put, ChangeOp::Put)
            | (EventMask::Delete, ChangeOp::Delete)
    )
}
