use serde::{Deserialize, Serialize};

use crate::client::session::entry::resume_security::{
    deserialize_resume_ticket, ProtectedResumeBuffer,
};

#[derive(Serialize, Deserialize)]
pub(crate) struct WireAuthInit {
    pub user: String,
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>,
    pub binding_mode: u8,
    pub version: u8,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireChallenge {
    #[serde(with = "serde_bytes")]
    pub salt: Vec<u8>,
    pub memory_kb: u32,
    pub time: u32,
    pub parallelism: u32,
    pub argon2_version: u8,
    #[serde(with = "serde_bytes")]
    pub server_nonce: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireClientProof {
    #[serde(with = "serde_bytes")]
    pub client_proof: Vec<u8>,
}

/// Sent by the client as the first frame when resuming a session via ticket.
#[derive(Serialize)]
pub(crate) struct WireResumeInit<'a> {
    #[serde(with = "serde_bytes")]
    pub ticket: &'a [u8],
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>,
    pub binding_mode: u8,
}

/// Sent by the client as the FIRST frame when resuming with identity
/// verification (RV-CLIENT-DE75ADDD): asks the server to prove its pinned
/// Ed25519 identity before the bearer ticket is disclosed.
#[derive(Serialize)]
pub(crate) struct WireResumeProofInit {
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>,
}

/// Server answer to [`WireResumeProofInit`]: current identity pub key plus
/// an Ed25519 signature over `RESUME_PROOF_V1 || client_nonce || exporter`.
#[derive(Deserialize)]
pub(crate) struct WireResumeProofOk {
    #[serde(with = "serde_bytes")]
    pub server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

/// Server response to a successful [`WireResumeInit`].
#[derive(Deserialize)]
pub(crate) struct WireResumeOk {
    #[serde(with = "serde_bytes")]
    pub session_id: Vec<u8>,
    pub expires_at_ns: u64,
    #[serde(default, deserialize_with = "deserialize_resume_ticket")]
    pub resumption_ticket: ProtectedResumeBuffer,
    #[serde(default)]
    pub resumption_expires_at_ns: u64,
    /// Max query-language version this server supports; `0` when absent
    /// (old server). Client emits v2 id-keyed write/read only when >= 2.
    #[serde(default)]
    pub server_query_version: u8,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireAuthOk {
    #[serde(with = "serde_bytes")]
    pub server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub session_id: Vec<u8>,
    pub expires_at_ns: u64,
    /// Resumption ticket — empty `Vec` if the server didn't issue one.
    #[serde(default, with = "serde_bytes")]
    pub resumption_ticket: Vec<u8>,
    #[serde(default)]
    pub resumption_expires_at_ns: u64,
    /// Max query-language version this server supports; `0` when absent
    /// (old server). Client emits v2 id-keyed write/read only when >= 2.
    #[serde(default)]
    pub server_query_version: u8,
    /// Presently informational; identity recovery requires explicit policy.
    #[serde(default)]
    pub rotation_in_progress: Option<shamir_connect::common::wire::RotationInProgressWire>,
    /// Informational KDF-upgrade request from the authenticated server.
    #[serde(default)]
    pub kdf_upgrade_required: Option<bool>,
}
