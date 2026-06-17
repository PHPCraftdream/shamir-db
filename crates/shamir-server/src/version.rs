//! Server-side version dispatch for the auth handshake protocol AND the
//! application-layer query language.
//!
//! Two independent version axes:
//!
//! - **Handshake protocol version** (`u8`) — carried in `auth_init.version`.
//!   Bumped when the SCRAM-Argon2id wire shape changes (new fields in
//!   `auth_init`/`challenge`/`auth_ok`, new binding-mode tag, etc.). The
//!   client refuses to talk to a server that does not support its requested
//!   version, and the server rejects unknown versions before doing any
//!   Argon2id work.
//!
//! - **Query language version** (`u32`) — carried as `query_version` in
//!   `DbRequest::Execute`. Bumped when the [`shamir_db::query::batch`]
//!   schema gains incompatible fields or removes existing ones. The handler
//!   rejects unknown versions before invoking `ShamirDb::execute` so a
//!   future-version client gets a typed error rather than a confusing
//!   "missing field" deserialization failure.
//!
//! Both lists are hardcoded — there is no run-time configuration to relax
//! them. Adding a new version means a code change here.

use thiserror::Error;

/// Hardcoded list of handshake protocol versions this server understands.
///
/// Order is informational: the server accepts any version present in this
/// slice. To support multiple in parallel (during a transition), include
/// both — the per-version branching is the responsibility of the
/// connection orchestration code that consumes `init.version`.
pub const SUPPORTED_HANDSHAKE_PROTO_VERSIONS: &[u8] = &[
    // v1: current version, matches `shamir_connect::common::types::ProtocolVersion::V1`.
    1,
];

/// The handshake protocol version the server prefers to negotiate.
/// (Today: identical to the only entry of [`SUPPORTED_HANDSHAKE_PROTO_VERSIONS`].)
pub const CURRENT_HANDSHAKE_PROTO_VERSION: u8 = 1;

/// Hardcoded list of query-language versions this server understands.
///
/// `u32` rather than `u8` because the query-language version is much more
/// likely to evolve than the wire-level handshake — easier to bump for a
/// long time without overflowing.
pub const SUPPORTED_QUERY_LANG_VERSIONS: &[u32] = &[
    // v1: original `shamir_db::query::batch::BatchRequest` shape.
    1,
    // v2: server now supports MessagePack id-keyed write/read pass-through.
    // Advertised via `auth_ok.server_query_version`; clients opt in to v2
    // behaviour only when they see that field >= 2. The request schema is
    // backward-compatible: a v2 client sending v2 `query_version` is treated
    // identically to v1 until the pass-through path is wired in (S-client).
    2,
];

/// The query-language version the server prefers to advertise.
pub const CURRENT_QUERY_LANG_VERSION: u32 = 2;

/// Version-mismatch error.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VersionError {
    /// Client asked for a handshake-protocol version this server does not
    /// implement.
    #[error(
        "handshake_protocol_version: client requested {requested}, server supports {supported:?}"
    )]
    UnsupportedHandshake {
        requested: u8,
        supported: &'static [u8],
    },
    /// Client asked for a query-language version this server does not
    /// implement.
    #[error("query_lang_version: client requested {requested}, server supports {supported:?}")]
    UnsupportedQueryLang {
        requested: u32,
        supported: &'static [u32],
    },
}

/// Reject any handshake-protocol version not in
/// [`SUPPORTED_HANDSHAKE_PROTO_VERSIONS`].
///
/// MUST be called BEFORE any Argon2id work — version mismatch is a fast
/// reject path, not an authentication-time decision.
#[inline]
pub fn check_handshake_proto(version: u8) -> Result<(), VersionError> {
    if SUPPORTED_HANDSHAKE_PROTO_VERSIONS.contains(&version) {
        Ok(())
    } else {
        Err(VersionError::UnsupportedHandshake {
            requested: version,
            supported: SUPPORTED_HANDSHAKE_PROTO_VERSIONS,
        })
    }
}

/// Reject any query-language version not in
/// [`SUPPORTED_QUERY_LANG_VERSIONS`].
#[inline]
pub fn check_query_lang(version: u32) -> Result<(), VersionError> {
    if SUPPORTED_QUERY_LANG_VERSIONS.contains(&version) {
        Ok(())
    } else {
        Err(VersionError::UnsupportedQueryLang {
            requested: version,
            supported: SUPPORTED_QUERY_LANG_VERSIONS,
        })
    }
}
