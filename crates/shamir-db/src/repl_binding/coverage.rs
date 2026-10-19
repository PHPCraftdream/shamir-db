//! Quarantine coverage derived from the durable subscription bindings
//! (S5c-X PR8b-2). No global sentinel exists: a valid row's coverage is
//! exactly its bound destination set, and a row that cannot be parsed is a
//! typed error — never a wildcard fallback.

use crate::repl_binding::binding::SubscriptionBinding;
use crate::types::value::QueryValue;
use crate::{DbResult, ShamirDb};

/// S5c-X PR8b: a subscription row is quarantined while `state ==
/// "resync_required"` OR while the durable `quarantined` marker is set. The
/// marker is written by [`ShamirDb::mark_subscription_terminal`] and survives
/// state changes that do not reseed (Pause → `"paused"`, SetProfile), so a
/// state flip alone can never lift the read gate.
pub(crate) fn row_is_quarantined(row: &QueryValue) -> bool {
    row.get("state").and_then(|value| value.as_str()) == Some("resync_required")
        || row.get("quarantined") == Some(&QueryValue::Bool(true))
}

/// One cache key per bound destination of `binding`; `reason` is used as
/// given (falling back to the row's `status_message` when empty).
pub(crate) fn quarantined_row_keys(
    row: &QueryValue,
    binding: &SubscriptionBinding,
    reason: &str,
) -> Vec<((String, String), String)> {
    let reason = if reason.is_empty() {
        row.get("status_message")
            .and_then(|value| value.as_str())
            .unwrap_or("subscription requires resync")
    } else {
        reason
    };
    binding
        .destinations()
        .iter()
        .map(|(db, repo)| ((db.clone(), repo.clone()), reason.to_string()))
        .collect()
}

impl ShamirDb {
    /// Single source of truth for the quarantine set: walk the
    /// `system/subscriptions` table rows that are quarantined
    /// ([`row_is_quarantined`]) and emit one `(db, repo)` key per bound
    /// destination, valued with the row's `status_message` (or a constant
    /// reason when absent).
    ///
    /// Coverage comes ONLY from the row's durable binding — no profile
    /// resolution and no sentinel fallback. A quarantined row whose binding
    /// cannot be parsed propagates the typed catalogue error.
    pub(crate) async fn resync_quarantine_entries(
        &self,
    ) -> DbResult<Vec<((String, String), String)>> {
        let mut entries = Vec::new();
        for row in crate::repl_binding::validate::read_subscription_rows(self).await? {
            if !row_is_quarantined(&row) {
                continue;
            }
            let reason = row
                .get("status_message")
                .and_then(|value| value.as_str())
                .unwrap_or("subscription requires resync");
            let binding = SubscriptionBinding::from_row(&row)?;
            entries.extend(quarantined_row_keys(&row, &binding, reason));
        }
        Ok(entries)
    }
}
