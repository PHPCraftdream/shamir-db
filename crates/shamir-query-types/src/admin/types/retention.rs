//! Temporal retention types and history-related DDL operations.

use serde::{Deserialize, Serialize};

fn default_repo() -> String {
    "main".to_string()
}

/// Per-table history retention. Three ORTHOGONAL optional knobs, each set
/// independently. All-`None` = Forever; `max_count: Some(0)` = CurrentOnly.
/// Caps intersect (tighter prunes); the floor (`min_count`) overrides
/// `max_age`. See `TEMPORAL.md` §3.
///
/// `Default` = all-`None` = Forever; the engine treats "table created
/// without retention" as CurrentOnly — that default lives at the table
/// layer in T3, not here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Retention {
    /// CAP by age (seconds). `None` = no age cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<u64>,
    /// CAP by version count/key. `None` = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_count: Option<u64>,
    /// FLOOR: always keep >= this many recent versions, past `max_age`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_count: Option<u64>,
}

impl Retention {
    /// CurrentOnly — no history is retained.
    pub fn current_only() -> Self {
        Self {
            max_count: Some(0),
            ..Default::default()
        }
    }

    /// Validate that `min_count <= max_count` when both are set.
    pub fn validate(&self) -> Result<(), String> {
        match (self.min_count, self.max_count) {
            (Some(min), Some(max)) if min > max => Err(format!(
                "retention min_count ({min}) must be <= max_count ({max})"
            )),
            _ => Ok(()),
        }
    }

    /// True when this retention means "keep no history at all" —
    /// `max_count == Some(0)` with no age floor or floor override.
    pub fn is_current_only(&self) -> bool {
        matches!(self.max_count, Some(0)) && self.max_age_secs.is_none() && self.min_count.is_none()
    }
}

/// Imperative history purge scope — the manual twin of vacuum.
/// Both predicates are epoch-millis / age-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PurgeScope {
    /// Purge history older than this timestamp (epoch-millis).
    OlderThan { timestamp: u64 },
    /// Purge history older than this age (seconds).
    OlderThanAge { age_secs: u64 },
}

/// Imperative history purge for a table.
///
/// ```json
/// { "purge_history": "users", "repo": "main", "scope": { "older_than_age": { "age_secs": 86400 } } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurgeHistoryOp {
    pub purge_history: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub scope: PurgeScope,
}

/// One-shot "changes since version V" read (temporal T4-changes-since).
///
/// A read-style admin op that returns the durable-journal events committed
/// STRICTLY AFTER the client's cursor `changes_since` (i.e. events with
/// `commit_version > changes_since`), plus the CF-1 gap marker. This is the
/// queryable foundation of #201 (live subscriptions); the live server-push
/// transport is a separate, larger piece.
///
/// ```json
/// { "changes_since": 0, "repo": "main", "limit": 1000 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangesSinceOp {
    /// Cursor: return events with `commit_version > this` (discriminator key).
    pub changes_since: u64,
    #[serde(default = "default_repo")]
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// Change a live table's history-retention policy on the fly (T3).
///
/// The discriminator key `set_retention` holds the table name. The
/// policy is applied via a lock-free `ArcSwap` swap — no data migration,
/// no reshape; subsequent writes are governed by the new policy.
///
/// ```json
/// { "set_retention": "users", "repo": "main", "retention": { "max_count": 5 } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetRetentionOp {
    pub set_retention: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub retention: Retention,
}
