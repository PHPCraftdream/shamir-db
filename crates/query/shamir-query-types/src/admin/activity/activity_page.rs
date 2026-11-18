//! [`ActivityPage`] — the `ListActivity` response page (R4b step 1, §5.1).

use crate::admin::ActivityRow;
use serde::{Deserialize, Serialize};

/// One bounded page of the activity view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityPage {
    /// Rendered rows (at most the server's per-call cap).
    pub rows: Vec<ActivityRow>,
    /// Opaque continuation token (registry kind + last-seen key + snapshot
    /// sequence); `None` when the listing is exhausted. A momentarily torn
    /// snapshot is acceptable and documented (copy-on-iterate, D4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<String>,
}

impl ActivityPage {
    /// Convenience constructor used by the server renderer and tests.
    pub fn new(rows: Vec<ActivityRow>, continuation: Option<String>) -> Self {
        Self { rows, continuation }
    }
}
