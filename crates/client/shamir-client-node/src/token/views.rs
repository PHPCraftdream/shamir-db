use serde::{Serialize, Serializer};

use shamir_client as core;
use zeroize::Zeroizing;

// The native copy is wiped on drop, but the msgpack `Buffer` handed to JS and
// the resulting JS string cannot be wiped.
fn serialize_token<S: Serializer>(
    token: &Zeroizing<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(token)
}

fn serialize_id<S: Serializer>(id: &[u8; 16], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(id)
}

/// Named-msgpack shape of a created API token; keys are the field names.
#[derive(Serialize)]
pub(crate) struct CreatedTokenView {
    pub(crate) id: String,
    #[serde(serialize_with = "serialize_token")]
    pub(crate) token: Zeroizing<String>,
    pub(crate) label: String,
    pub(crate) owner: String,
    pub(crate) mode: core::ApiTokenMode,
    pub(crate) channels: core::ApiTokenChannels,
    pub(crate) databases: Option<Vec<String>>,
    pub(crate) created_at_ns: u64,
    pub(crate) expires_at_ns: u64,
}

impl CreatedTokenView {
    pub(crate) fn from_created(created: core::CreatedApiToken) -> Self {
        Self {
            token: Zeroizing::new(created.token.reveal().to_owned()),
            id: created.id,
            label: created.label,
            owner: created.owner,
            mode: created.mode,
            channels: created.channels,
            databases: created.databases,
            created_at_ns: created.created_at_ns,
            expires_at_ns: created.expires_at_ns,
        }
    }
}

/// Named-msgpack shape of a password login result; `id` is a 16-byte bin.
#[derive(Serialize)]
pub(crate) struct LoginView {
    #[serde(serialize_with = "serialize_token")]
    pub(crate) token: Zeroizing<String>,
    #[serde(serialize_with = "serialize_id")]
    pub(crate) id: [u8; 16],
    pub(crate) owner: String,
    pub(crate) label: String,
    pub(crate) mode: core::LoginMode,
    pub(crate) channels: core::LoginChannels,
    pub(crate) databases: Option<Vec<String>>,
    pub(crate) created_at_ns: u64,
    pub(crate) expires_at_ns: u64,
}

impl LoginView {
    pub(crate) fn from_outcome(outcome: core::LoginOutcome) -> Self {
        Self {
            token: Zeroizing::new(outcome.token.reveal().to_owned()),
            id: outcome.id,
            owner: outcome.owner,
            label: outcome.scope.label,
            mode: outcome.scope.mode,
            channels: outcome.scope.channels,
            databases: outcome.scope.databases,
            created_at_ns: outcome.created_at_ns,
            expires_at_ns: outcome.expires_at_ns,
        }
    }
}
