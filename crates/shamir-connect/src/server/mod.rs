//! Server-side of the connection protocol.

pub mod config;
pub mod handshake;
pub mod ticket;
pub mod user_record;

pub use config::{ListenerPolicy, ServerSecrets};
pub use handshake::{
    AuthInitView, AuthOkView, ChallengeView, ProofOutcome, ServerHandshake, SESSION_MAX_AGE_NS,
};
pub use ticket::{
    check_anti_downgrade, decrypt_ticket, encrypt_ticket, validate_ticket_enums, TicketPlain,
    TicketWire,
};
pub use user_record::UserRecord;
