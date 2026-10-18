//! [`InternerDelta`] — the per-repo epoch-delta payload carried in a
//! [`BatchResponse`](super::BatchResponse) for ambient interner cache sync
//! (Stage 5-wire, Part A).
//!
//! Each entry maps a repo name to the server's delta beyond the client's
//! advertised epoch: the server's new gap-free high-water `epoch` plus the
//! `(id, name)` entries the client does not yet have. Backward-compatible:
//! both sides default to empty, so old peers are unaffected.

use serde::{Deserialize, Serialize};

/// Per-repo interner delta returned by the server in
/// [`BatchResponse::interner_delta`](super::BatchResponse::interner_delta).
///
/// - `epoch` — the server's new gap-free high-water id after capturing the
///   delta (from `Interner::entries_after`'s `new_high_water`). The client
///   CAS-maxes its own epoch against this.
/// - `entries` — `(id, name)` pairs the client does not yet have (id > its
///   advertised epoch). Id-first to match `interner_dump`'s `entries` shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternerDelta {
    pub epoch: u64,
    pub entries: Vec<(u64, String)>,
}
