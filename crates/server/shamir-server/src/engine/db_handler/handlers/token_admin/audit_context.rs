use crate::security::tokens::TokenAuditContext;
use shamir_connect::common::types::TransportKind;
use shamir_connect::server::session::Session;

pub(super) fn audit_context(session: &Session) -> TokenAuditContext {
    let transport = match session.transport_kind {
        TransportKind::Tcp => "tcp",
        TransportKind::WebSocket => "ws",
        TransportKind::Unix => "unix",
    };
    let mut session_id_prefix = [0; 8];
    if let Some(prefix) = session.session_id.get(..8) {
        session_id_prefix.copy_from_slice(prefix);
    }
    TokenAuditContext {
        actor_username: session.username.clone(),
        transport: transport.to_owned(),
        session_id_prefix,
    }
}
