//! Network-edge protections: TLS material and the response byte budget.

/// Shared response-byte budget and reservations.
pub mod byte_budget;
/// REST bearer-token authentication and execution.
pub mod http_rest;
pub mod tls;
