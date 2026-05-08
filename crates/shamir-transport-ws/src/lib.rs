//! WebSocket transport binding for shamir-connect (spec TRANSPORT_WS).
//!
//! Two endpoint modes per spec TRANSPORT_WS §2:
//!
//! 1. **Native WSS** (`/shamir/v1`, `binding_mode = 0x01`): TLS 1.3 with
//!    exporter-based channel binding. Used by native clients (Rust, Go,
//!    Python) that can access the TLS exporter.
//!
//! 2. **Browser WSS** (`/shamir/v1/browser`, `binding_mode = 0x02`):
//!    TLS 1.3 without exporter (browser JS can't access it). Channel
//!    binding falls back to a 32-byte zero placeholder; the protocol
//!    accepts this as a strictly weaker binding mode (anti-downgrade
//!    matrix in spec §6.4 prevents resumption from upgrading binding
//!    strength). MUST validate the `Origin` header per spec §9.
//!
//! ## Frame format
//!
//! Each WebSocket BINARY message carries one length-prefixed
//! shamir-transport-tcp-style frame: `[u32_be length][payload]`. We reuse
//! the framing crate's encoder/decoder via [`ws_send`]/[`ws_recv`] adapters
//! that operate on a `WebSocketStream`'s message boundary.

pub mod browser;
pub mod framing;
pub mod server;
pub mod tls_exporter;

pub use browser::{validate_origin, BrowserOriginPolicy, OriginRejected};
pub use framing::{ws_recv, ws_recv_into, ws_send, WsFrameError};
pub use server::{accept_native_ws, accept_browser_ws, WsAcceptError};
pub use tls_exporter::extract_tls_exporter_from_stream;
