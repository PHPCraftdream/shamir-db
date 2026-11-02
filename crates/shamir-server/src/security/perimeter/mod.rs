//! Network-edge protections: TLS material and the response byte budget.

/// Shared response-byte budget and reservations.
pub mod byte_budget;
/// REST bearer-token authentication and execution.
pub mod http_rest;
/// HTTP WebSocket channel security and admission.
pub mod http_ws;
pub mod tls;
