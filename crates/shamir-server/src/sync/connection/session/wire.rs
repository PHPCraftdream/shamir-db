//! Wire view of `auth_init`, `challenge`, `client_proof`, `auth_ok` —
//! these match the shapes used by the transport-tcp e2e test, kept
//! transport-binding-local.

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use shamir_connect::common::wire::RotationInProgressWire;

#[derive(Serialize, Deserialize)]
pub struct AuthInit {
    pub user: String,
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>,
    pub binding_mode: u8,
    pub version: u8,
}

#[derive(Serialize, Deserialize)]
pub struct Challenge {
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
pub struct ClientProof {
    #[serde(with = "serde_bytes")]
    pub client_proof: Vec<u8>,
}

#[derive(Deserialize)]
pub struct AuthOk {
    #[serde(with = "serde_bytes")]
    pub server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub session_id: Vec<u8>,
    pub expires_at_ns: u64,
    /// Optional resumption ticket — when present, the client may
    /// reconnect later (within the TTL) without re-running Argon2id.
    /// Wire-encoded form per spec §5.4 / SESSION_RESUMPTION.
    /// Always present on the wire (empty Vec when no ticket issued);
    /// positional msgpack — omitting a field shifts array indices.
    #[serde(default, with = "serde_bytes")]
    pub resumption_ticket: Vec<u8>,
    /// Absolute (unix nanos) expiry of the ticket above. `0` when no
    /// ticket was issued. Always present (positional msgpack).
    #[serde(default)]
    pub resumption_expires_at_ns: u64,
    /// Max query-language version this server supports. `0` means the
    /// server predates query-lang negotiation. Always present on the wire
    /// (positional msgpack — omitting a non-trailing field shifts array
    /// indices and breaks the client decode).
    #[serde(default)]
    pub server_query_version: u8,
    /// Orphan recovery payload during an identity-rotation overlap.
    #[serde(default)]
    pub rotation_in_progress: Option<RotationInProgressWire>,
    /// Ask the client to upgrade stored KDF parameters when necessary.
    #[serde(default)]
    pub kdf_upgrade_required: Option<bool>,
}

impl Serialize for AuthOk {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let has_hints = self.rotation_in_progress.is_some() || self.kdf_upgrade_required.is_some();
        let mut state = serializer.serialize_struct("AuthOk", if has_hints { 10 } else { 8 })?;
        state.serialize_field(
            "server_signature",
            serde_bytes::Bytes::new(&self.server_signature),
        )?;
        state.serialize_field(
            "server_pub_key",
            serde_bytes::Bytes::new(&self.server_pub_key),
        )?;
        state.serialize_field("identity_sig", serde_bytes::Bytes::new(&self.identity_sig))?;
        state.serialize_field("session_id", serde_bytes::Bytes::new(&self.session_id))?;
        state.serialize_field("expires_at_ns", &self.expires_at_ns)?;
        state.serialize_field(
            "resumption_ticket",
            serde_bytes::Bytes::new(&self.resumption_ticket),
        )?;
        state.serialize_field("resumption_expires_at_ns", &self.resumption_expires_at_ns)?;
        state.serialize_field("server_query_version", &self.server_query_version)?;
        if has_hints {
            state.serialize_field("rotation_in_progress", &self.rotation_in_progress)?;
            state.serialize_field("kdf_upgrade_required", &self.kdf_upgrade_required)?;
        }
        state.end()
    }
}

/// Client → server first frame when attempting a session resume.
/// Carries the opaque ticket from the previous `auth_ok` plus a fresh
/// client nonce.
#[derive(Serialize, Deserialize)]
pub struct ResumeInit {
    #[serde(with = "serde_bytes")]
    pub ticket: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>,
    pub binding_mode: u8,
}

/// Client → server OPTIONAL resume preamble (RV-CLIENT-DE75ADDD): asks the
/// server to prove its pinned Ed25519 identity BEFORE the bearer ticket is
/// disclosed. Distinguished from `ResumeInit` / `AuthInit` by its 1-field
/// positional msgpack shape.
#[derive(Serialize, Deserialize)]
pub struct ResumeProofInit {
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>,
}

/// Server → client answer to [`ResumeProofInit`]: the CURRENT identity pub
/// key plus an Ed25519 signature over
/// `RESUME_PROOF_V1 || client_nonce || tls_exporter`.
#[derive(Serialize, Deserialize)]
pub struct ResumeProofOkWire {
    #[serde(with = "serde_bytes")]
    pub server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

/// Server → client response for a successful resume.
/// A subset of `AuthOk` — the client already has the server's Ed25519
/// pub-key and signature from the original SCRAM handshake.
#[derive(Serialize, Deserialize)]
pub struct ResumeOkWire {
    #[serde(with = "serde_bytes")]
    pub session_id: Vec<u8>,
    pub expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    pub resumption_ticket: Vec<u8>,
    #[serde(default)]
    pub resumption_expires_at_ns: u64,
    /// Max query-language version this server supports. Always present on
    /// the wire (positional msgpack — see `AuthOk::server_query_version`).
    #[serde(default)]
    pub server_query_version: u8,
}
