//! Per-connection orchestration — TLS accept → optional WS upgrade →
//! pre-Argon2id binding-mode check → rate-limit → SCRAM handshake under
//! Argon2 semaphore + latency padding → lockout register/reset →
//! session insert with per-user cap → request loop with
//! `dispatch_request_view` → 5s grace + audit emit on terminal events.
//!
//! This module wires every security primitive defined elsewhere in
//! `shamir-connect` (lockout, rate_limit, argon2_semaphore,
//! latency, audit_chain, ServerHandshake, dispatch_request_view).

pub mod conn_limiter;

mod handshake;
mod push_sink;
mod request_loop;
mod session;
// `pub(crate)` so the in-crate adapter test (`src/tests/user_state_lookup_tests`)
// can name `RedbUserStateLookup`; production use stays internal to this module.
pub(crate) mod user_state_lookup;

#[cfg(test)]
pub(crate) use handshake::checked_query_version;
pub use handshake::handle_connection;
#[cfg(test)]
pub(crate) use handshake::{handshake_budget, CLOCK_OVERRIDE_NS};
pub use request_loop::request_loop;
#[cfg(test)]
pub(crate) use session::auth_ok_hints;
pub use session::ConnTasks;
pub use session::ConnectionContext;
pub(crate) use session::{in_flight_guard, wire};
