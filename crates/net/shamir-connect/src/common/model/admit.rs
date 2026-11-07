//! Admission-decision rejection reason + activity window, shared by the
//! COMMON layer so `common::error::Error::AdmissionRejected` can carry it
//! in client-only builds (no `server` feature needed). The full
//! [`crate::server::admission::Admission`] outcome type stays server-side.

/// Activity window: a session is ACTIVE iff
/// `now_ns.saturating_sub(last_activity_ns) < ACTIVE_WINDOW_NS`, idle
/// otherwise. Exactly 60 s is idle (spec §7.4).
pub const ACTIVE_WINDOW_NS: u64 = 60 * 1_000_000_000;

/// Internal reason an admission was rejected. Never leaves the server
/// process — the wire collapses it to a generic authentication failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmitReject {
    /// Every existing session of the user was active within the window
    /// (spec §7.4: reject the new login).
    AllActive,
    /// The session id is already bound to a DIFFERENT user's session.
    SidConflict,
}
