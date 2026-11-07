//! Admission decision types for [`crate::server::state::session::SessionStore::admit`]
//! (spec §7.4 overflow policy: idle-LRU eviction or all-active rejection).
//!
//! One admission function serves login AND resume (RV-CONNECT-8AD8B1C8).
//! `AdmitReject` + `ACTIVE_WINDOW_NS` live in the COMMON layer
//! (`crate::common::model::admit`) so `Error::AdmissionRejected` can carry
//! the reason in client-only builds; they are re-exported here so every
//! existing `server::admission::{...}` path stays stable.

use smallvec::SmallVec;
use std::sync::Arc;

use crate::common::types::limits;
use crate::server::state::session::Session;

pub use crate::common::admit::{AdmitReject, ACTIVE_WINDOW_NS};

/// Session-id key type (bearer token).
pub type Sid = [u8; limits::SESSION_ID_BYTES];

/// Outcome of [`crate::server::state::session::SessionStore::admit`].
#[derive(Debug)]
pub enum Admission {
    /// Session admitted. `evicted` lists the idle LRU victims removed to
    /// make room; `replaced` is true when the same sid was re-admitted for
    /// the same user (a replacement consumes no extra capacity).
    Admitted {
        /// The admitted session.
        session: Arc<Session>,
        /// Sids evicted (idle LRU, spec §7.4).
        evicted: SmallVec<[Sid; 1]>,
        /// Same sid + same user re-admission.
        replaced: bool,
    },
    /// Rejected — mutates nothing apart from idempotent index pruning.
    Rejected(AdmitReject),
}
