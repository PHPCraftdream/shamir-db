//! Server-side of the connection protocol.
//!
//! Modules are grouped into topical subfolders; every module is re-exported
//! here so its public path stays `shamir_connect::server::<module>`.

#[cfg(test)]
mod tests;

mod auth;
mod infra;
mod limits;
mod ops;
mod state;

pub use auth::{bootstrap, changepw, handshake};
#[cfg(feature = "durable-fjall")]
pub use infra::durable_counters;
pub use infra::{audit_chain, config, conn_services, dispatch};
pub use limits::{argon2_semaphore, lockout, rate_limit};
pub use ops::{admin, rotation, user_record};
pub use state::{admission, resume, session, ticket};

pub use conn_services::{ConnectionServices, PushRejected, PushSink};
pub use dispatch::{dispatch_request, DispatchOutcome, RequestHandler};
pub use session::{PendingChangePwChallenge, Session, SessionPermissions, SessionStore};
pub use state::admission::{Admission, AdmitReject, Sid, ACTIVE_WINDOW_NS};

pub use config::{ListenerPolicy, ServerSecrets};
pub use handshake::{
    AuthInitView, AuthOkView, ChallengeView, ProofOutcome, ServerHandshake, SESSION_MAX_AGE_NS,
};
pub use ticket::{
    check_anti_downgrade, decrypt_ticket, encrypt_ticket, validate_ticket_enums, TicketPlain,
    TicketWire,
};
pub use user_record::UserRecord;
