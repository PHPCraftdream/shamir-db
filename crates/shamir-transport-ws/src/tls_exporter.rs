//! TLS exporter extraction for native WSS endpoint (binding_mode = 0x01).
//!
//! Reuses the same exporter label as the TCP transport
//! (`EXPORTER-ShamirDB-AUTH-v1`, RFC 9266) so a native client connecting
//! over WSS computes the same channel-binding bytes as if it had
//! connected over raw TLS. This is what makes resumption tickets
//! cross-transport (TCP ↔ WSS) per spec §6.4.
//!
//! For browser WSS (binding_mode = 0x02) the exporter is NOT extracted —
//! browser JS can't access it, so the protocol accepts the strictly
//! weaker `[0u8; 32]` placeholder.

use shamir_transport_tcp::tls::{extract_tls_exporter, ConnectionExporter};

/// Extract the 32-byte TLS exporter from any compatible TLS stream.
/// Returns `None` if extraction fails (e.g., handshake not complete).
///
/// Intended for use on native WSS endpoints where the underlying stream
/// type is `tokio_rustls::server::TlsStream<TcpStream>`.
pub fn extract_tls_exporter_from_stream(connection: &impl ConnectionExporter) -> Option<[u8; 32]> {
    extract_tls_exporter(connection)
}

/// Placeholder for browser WSS where exporter is unavailable.
pub const BROWSER_CHANNEL_BINDING: [u8; 32] = [0u8; 32];
