//! Token-over-REST transport: endpoint description, TLS trust and the
//! HTTP/1.1 connection.

mod endpoint;
mod http_conn;
mod status;
mod tls;

#[cfg(test)]
mod tests;

pub use endpoint::TokenEndpoint;
pub use tls::TokenTrust;

pub(crate) use endpoint::TokenScheme;
#[cfg(test)]
pub(crate) use http_conn::TokenReply;
pub(crate) use http_conn::TokenTransport;
pub(crate) use status::{map_ping_reply, map_reply};
pub(crate) use tls::{build_connector, parse_server_name};
