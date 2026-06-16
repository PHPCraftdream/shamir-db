//! Wire DTOs for the per-repo interner introspection / registration ops.
//!
//! Both ops use the repo name as the discriminator key (mirrors the
//! `changes_since` / `purge_history` convention from `retention.rs`):
//! the interner lives on `RepoInstance`, so the repo is the routing
//! dimension, not a table.

use serde::{Deserialize, Serialize};

fn default_repo() -> String {
    "main".to_string()
}

/// Dump a repo's interner dictionary (id → name).
///
/// Without `since`: return the whole dictionary plus the current epoch
/// (the highest gap-free id present). With `since`: return only the
/// entries whose id is strictly greater than `since` (delta refresh) —
/// the client caches the full dict locally and only pulls the tail.
///
/// ```json
/// { "interner_dump": "main" }
/// { "interner_dump": "main", "since": 12 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternerDumpOp {
    /// Repo whose interner to dump (discriminator key = repo name).
    #[serde(default = "default_repo")]
    pub interner_dump: String,
    /// Optional cursor: only return entries with id > `since`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
}

/// Register field NAMES, returning the (name → id) mapping.
///
/// Names are interned idempotently — a name already present returns its
/// existing id, a new name mints the next id. §9.4 invariant: a key is
/// ALWAYS a name; the string `"42"` interns to whatever id the interner
/// assigns (almost certainly NOT 42) — the interner is the sole id
/// authority.
///
/// ```json
/// { "interner_touch": "main", "names": ["age", "name", "42"] }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternerTouchOp {
    /// Repo whose interner to touch (discriminator key = repo name).
    #[serde(default = "default_repo")]
    pub interner_touch: String,
    /// Field names to intern. MUST be `Vec<String>` — never numeric ids.
    pub names: Vec<String>,
}
