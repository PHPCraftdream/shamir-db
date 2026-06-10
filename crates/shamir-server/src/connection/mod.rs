//! Per-connection orchestration — TLS accept → optional WS upgrade →
//! pre-Argon2id binding-mode check → rate-limit → SCRAM handshake under
//! Argon2 semaphore + latency padding → lockout register/reset →
//! session insert with per-user cap → request loop with
//! `dispatch_request_view` → 5s grace + audit emit on terminal events.
//!
//! This module wires every security primitive defined elsewhere in
//! `shamir-connect` (lockout, rate_limit, argon2_semaphore,
//! latency, audit_chain, ServerHandshake, dispatch_request_view).

mod connection_context;
mod in_flight_guard;
mod user_state_lookup;

pub use connection_context::{handle_connection, request_loop, ConnectionContext};
