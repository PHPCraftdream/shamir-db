/// Identity and transport metadata attached to token administration audit events.
#[derive(Clone, Debug)]
pub struct TokenAuditContext {
    /// Username of the actor who initiated the operation, when known.
    pub actor_username: String,
    /// Transport label; service-only calls default to `token`.
    pub transport: String,
    /// Eight-byte prefix of the initiating session id, or all zeroes when unavailable.
    pub session_id_prefix: [u8; 8],
}
impl Default for TokenAuditContext {
    fn default() -> Self {
        Self::service_default()
    }
}
impl TokenAuditContext {
    /// Context for service calls without a session actor.
    pub fn service_default() -> Self {
        Self {
            actor_username: String::new(),
            transport: "token".to_owned(),
            session_id_prefix: [0; 8],
        }
    }
}
