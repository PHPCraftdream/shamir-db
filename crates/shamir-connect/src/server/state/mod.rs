//! Session lifecycle: admission types, session store, resume flow, tickets.

pub mod admission;
/// Authentication provenance retained on each server session.
pub mod origin;
pub mod resume;
pub mod session;
pub mod ticket;
/// Admission and accounting for token-authenticated sessions.
pub mod token_session;
