//! [`EdgeKind`] — provenance tag for a dependency edge in [`super::BatchPlan`].
//!
//! An edge `alias -> dep_alias` in the planner's dependency graph can arise
//! from two independent sources:
//! - an explicit `after: [dep_alias]` entry (pure ordering, no data access
//!   intent), or
//! - an auto-extracted `$query` reference (`FilterValue::QueryRef`, or a
//!   `$query` key inside a value/bind map) — a genuine data-flow dependency.
//!
//! The same pair of aliases can carry both at once (an `after` that happens
//! to name an alias already referenced via `$query`) — that's not an error,
//! just a redundant ordering hint layered on top of a real data dependency.
//! [`EdgeKind::Both`] records that case so downstream consumers (resolved-refs
//! construction, diagnostics, wire responses) can tell provenance apart
//! without re-deriving it.

use serde::{Deserialize, Serialize};

/// Where a dependency edge in the batch plan came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Declared purely via `after: [...]` — ordering only, no data flow.
    /// The dependent op's `resolved_refs` must NOT include this alias's
    /// result on account of this edge alone.
    Explicit,

    /// Auto-extracted from a `$query` reference — a genuine data
    /// dependency. The dependent op's `resolved_refs` includes this
    /// alias's result.
    DataFlow,

    /// Both an explicit `after` AND an auto-extracted `$query` reference
    /// point at the same alias. Data access is granted (via the `DataFlow`
    /// half); the `after` half is a redundant ordering hint.
    Both,
}

impl EdgeKind {
    /// True if this edge carries an `after`-declared ordering constraint
    /// (`Explicit` or `Both`).
    pub fn is_explicit(self) -> bool {
        matches!(self, EdgeKind::Explicit | EdgeKind::Both)
    }

    /// True if this edge carries a real `$query` data dependency
    /// (`DataFlow` or `Both`).
    pub fn is_data_flow(self) -> bool {
        matches!(self, EdgeKind::DataFlow | EdgeKind::Both)
    }

    /// Merge two provenance tags for the same edge (dedup with both flags
    /// preserved).
    pub fn merge(self, other: EdgeKind) -> EdgeKind {
        if self == other {
            return self;
        }
        EdgeKind::Both
    }
}
