//! Server-side post-handshake request dispatch.
//!
//! Implements the per-request session validity check (spec §7.5 [NORMATIVE])
//! and provides a single entry point for transport bindings to feed in
//! decoded request envelopes and receive response/error envelopes back.
//!
//! Application-level dispatch (what to do with `req` payload) is left to
//! the caller via the [`RequestHandler`] trait.

use crate::common::envelope::{
    ErrorEnvelope, RequestEnvelope, RequestEnvelopeView, ResponseEnvelope,
};
use crate::common::error::Result;
use crate::server::session::{Session, SessionStore};
use std::sync::Arc;

/// Application-level request handler — receives the validated [`Session`] and
/// raw `req` bytes, returns either response bytes or an error string.
pub trait RequestHandler: Send + Sync {
    /// Handle a single validated request.
    ///
    /// Returning `Ok(bytes)` → wrap in [`ResponseEnvelope`].
    /// Returning `Err(reason)` → wrap in [`ErrorEnvelope`].
    fn handle(&self, session: &Session, req: &[u8]) -> std::result::Result<Vec<u8>, String>;
}

/// Lookup hook: returns `tickets_invalid_before_ns` for a given user_id.
/// Caller wires this to whatever store backs `__system__/users`.
pub type TicketsInvalidBeforeLookup = dyn Fn(&[u8; 16]) -> u64 + Send + Sync;

/// Outcome of [`dispatch_request`].
#[derive(Debug)]
pub enum DispatchOutcome {
    /// Wire-ready response.
    Response(ResponseEnvelope),
    /// Wire-ready error envelope. Caller decides whether to also drop
    /// the underlying transport (e.g. for `session_invalidated`).
    Error(ErrorEnvelope),
}

/// Per-request entry point.
///
/// 1. Parse `session_id` from the envelope.
/// 2. Look up the session in `store`. Missing → `session_expired`.
/// 3. Run spec §7.5 validity check via `lookup_tickets_invalid_before_ns`.
///    Failure → remove session from store + return `session_invalidated`.
/// 4. Touch `last_activity_ns` (done by `SessionStore::lookup`).
/// 5. Dispatch `req` bytes to `handler`.
pub fn dispatch_request<H: RequestHandler, F: Fn(&[u8; 16]) -> u64>(
    envelope: &RequestEnvelope,
    store: &SessionStore,
    lookup_tickets_invalid_before_ns: F,
    handler: &H,
) -> Result<DispatchOutcome> {
    let sid = envelope.session_id_array()?;

    let session: Arc<Session> = match store.lookup(&sid) {
        Some(s) => s,
        None => {
            return Ok(DispatchOutcome::Error(ErrorEnvelope::new(
                envelope.request_id,
                "session_expired",
            )));
        }
    };

    // Per-spec §7.5 [NORMATIVE]: kill stale sessions on next request.
    let user_invalid_before = lookup_tickets_invalid_before_ns(&session.user_id);
    if !session.is_valid_for_user(user_invalid_before) {
        // Remove from store immediately so concurrent requests can't reuse.
        store.remove(&sid);
        return Ok(DispatchOutcome::Error(ErrorEnvelope::new(
            envelope.request_id,
            "session_invalidated",
        )));
    }

    // Application-level dispatch.
    match handler.handle(&session, &envelope.req) {
        Ok(res_bytes) => Ok(DispatchOutcome::Response(ResponseEnvelope::ok(
            envelope.request_id,
            res_bytes,
        ))),
        Err(err) => Ok(DispatchOutcome::Error(ErrorEnvelope::new(
            envelope.request_id,
            err,
        ))),
    }
}

/// **Optim #4** zero-copy variant of [`dispatch_request`].
///
/// Takes a [`RequestEnvelopeView`] borrowed directly from a wire buffer —
/// no `Vec<u8>` allocation for `session_id` or `req`. Functionally
/// identical: same §7.5 validity check, same handler dispatch, same
/// outcome shape.
///
/// Use this on transport hot paths where you already hold the raw msgpack
/// bytes; pair with [`shamir-transport-tcp::framing::read_frame_into`] to
/// keep the entire request path allocation-free.
pub fn dispatch_request_view<H: RequestHandler, F: Fn(&[u8; 16]) -> u64>(
    view: &RequestEnvelopeView<'_>,
    store: &SessionStore,
    lookup_tickets_invalid_before_ns: F,
    handler: &H,
) -> Result<DispatchOutcome> {
    let sid = view.session_id_array()?;

    let session: Arc<Session> = match store.lookup(sid) {
        Some(s) => s,
        None => {
            return Ok(DispatchOutcome::Error(ErrorEnvelope::new(
                view.request_id,
                "session_expired",
            )));
        }
    };

    let user_invalid_before = lookup_tickets_invalid_before_ns(&session.user_id);
    if !session.is_valid_for_user(user_invalid_before) {
        store.remove(sid);
        return Ok(DispatchOutcome::Error(ErrorEnvelope::new(
            view.request_id,
            "session_invalidated",
        )));
    }

    match handler.handle(&session, view.req) {
        Ok(res_bytes) => Ok(DispatchOutcome::Response(ResponseEnvelope::ok(
            view.request_id,
            res_bytes,
        ))),
        Err(err) => Ok(DispatchOutcome::Error(ErrorEnvelope::new(view.request_id, err))),
    }
}
