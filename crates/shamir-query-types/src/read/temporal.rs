//! Temporal read selectors — `At` and `Temporal`.
//!
//! Pure DTOs: the engine resolves `Timestamp` → version and serves the
//! read (T4). Here we only define the wire shapes.

use serde::{Deserialize, Serialize};

use super::OrderDirection;

/// A point in time for temporal reads. `Version` is exact and cheap;
/// `Timestamp` is resolved to a version by the engine (T4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum At {
    Version(u64),
    /// Epoch-millis — matches the representation the changefeed uses.
    Timestamp(u64),
}

/// Temporal selector on a [`super::ReadQuery`]. `Latest` = today's read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Temporal {
    /// Today's read — the default path, byte-identical to pre-temporal
    /// behaviour when skip-serialized.
    #[default]
    Latest,
    /// Point-in-time read at `at`.
    AsOf { at: At },
    /// Range read over history. `from`/`to` bound the window; either
    /// may be omitted for an open bound. `limit` caps the version
    /// count returned. `order` defaults to `Asc` (oldest → newest).
    History {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<At>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<At>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u64>,
        #[serde(default)]
        order: OrderDirection,
    },
}

impl Temporal {
    /// True when this is the default `Latest` variant — used by
    /// `skip_serializing_if` so the default path stays off the wire.
    pub fn is_latest(&self) -> bool {
        matches!(self, Temporal::Latest)
    }
}
