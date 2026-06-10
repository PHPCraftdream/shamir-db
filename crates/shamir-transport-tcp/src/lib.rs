//! TCP transport binding for shamir-connect (spec TRANSPORT_TCP.md).
//!
//! Provides TLS 1.3 (rustls) + length-prefix msgpack framing on top of
//! `tokio::net::TcpStream`. The protocol-level handshake (`ServerHandshake`,
//! `ClientHandshake`) is handled by the `shamir-connect` crate; this crate
//! only deals with bytes-on-the-wire concerns.

pub mod framing;
pub mod listener;
pub mod tls;

pub use framing::{read_frame, write_frame, FrameError, MAX_FRAME_SIZE_DEFAULT};
#[cfg(test)]
mod tests;

pub use tls::{
    extract_tls_exporter, generate_self_signed_server_cert, make_client_config_no_ca,
    make_server_config_from_pem,
};
