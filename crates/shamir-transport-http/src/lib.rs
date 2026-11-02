//! HTTP/1.1 server transport for generic request authentication and execution.

/// Shared request-body byte budget and reservations.
pub mod budget;
/// Listener configuration and endpoint limits.
pub mod config;
/// HTTP request framing validation.
pub mod framing;
/// Authentication, execution, and connection-admission interfaces.
pub mod gate;
/// HTTP connection acceptance and serving.
pub mod server;
#[cfg(test)]
mod tests;

pub use budget::{BodyBudget, BodyReservation};
pub use config::{Endpoint, EndpointSet, HttpLimits, HttpServerConfig};
pub use gate::{
    AuthHeader, ConnAdmission, ExecOutcome, ExecStatus, GateDecision, PeerInfo, RestExecutor,
    TokenGate,
};
pub use server::accept::{serve, ServeError};
