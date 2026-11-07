use shamir_connect::common::types::{BindingMode, TransportKind};

/// The link between the transport and the handshake axes: what the transport
/// guarantees about the channel, as fed to the SCRAM / resume exchange.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelBinding {
    pub(in crate::client) transport: TransportKind,
    pub(in crate::client) mode: BindingMode,
    pub(in crate::client) exporter: [u8; 32],
}

impl ChannelBinding {
    /// TCP + TLS: the binding is the RFC 5705 exporter of the TLS session.
    pub(crate) fn tcp_tls_exporter(exporter: [u8; 32]) -> Self {
        Self {
            transport: TransportKind::Tcp,
            mode: BindingMode::TlsExporter,
            exporter,
        }
    }

    /// WebSocket over TLS: the binding is the TLS-session exporter, extracted
    /// before the upgrade (identical value and label as on TCP).
    pub(crate) fn ws_tls_exporter(exporter: [u8; 32]) -> Self {
        Self {
            transport: TransportKind::WebSocket,
            mode: BindingMode::TlsExporter,
            exporter,
        }
    }

    /// WebSocket over TLS with the browser profile (`tls_no_export`): the
    /// TLS exporter is not part of the binding — zeros, per spec §6.4.
    pub(crate) fn ws_browser() -> Self {
        Self {
            transport: TransportKind::WebSocket,
            mode: BindingMode::TlsNoExport,
            exporter: [0u8; 32],
        }
    }

    /// Plain TCP (loopback only): no TLS, zeroed channel binding.
    pub(crate) fn tcp_plain() -> Self {
        Self {
            transport: TransportKind::Tcp,
            mode: BindingMode::None,
            exporter: [0u8; 32],
        }
    }

    /// Local IPC (Unix socket / Named Pipe): no TLS, zeroed channel binding.
    pub(crate) fn ipc() -> Self {
        Self {
            transport: TransportKind::Unix,
            mode: BindingMode::None,
            exporter: [0u8; 32],
        }
    }
}
