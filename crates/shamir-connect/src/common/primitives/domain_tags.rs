//! Domain separation tags (spec §17).
//!
//! All ASCII byte strings used as fixed prefixes in HMAC / HKDF / sign inputs.
//! Bumping protocol version requires renaming each tag (`-v2` etc) so that
//! cross-version cryptographic confusion is structurally impossible.

/// SCRAM client key derivation label (RFC 5802).
pub const CLIENT_KEY: &[u8] = b"Client Key";

/// SCRAM server key derivation label (RFC 5802).
pub const SERVER_KEY: &[u8] = b"Server Key";

/// HKDF salt for anti-enumeration `fake_blob` (spec §5.2.1).
pub const FAKE_SALT_V1: &[u8] = b"SHAMIR-FAKE-SALT-v1";

/// `auth_message` header (spec §4.1) — 14 bytes.
pub const AUTH_V1: &[u8] = b"SHAMIR-AUTH-v1";

/// `auth_message_cp` header for `changePassword` (spec §12.5).
pub const CHGPW_V1: &[u8] = b"SHAMIR-CHGPW-v1";

/// `identity_input` prefix (spec §5.2.4).
pub const IDENTITY_V1: &[u8] = b"SHAMIR-IDENTITY-v1";

/// Bootstrap challenge signature prefix (spec §11.3.3).
pub const BOOTSTRAP_V1: &[u8] = b"SHAMIR-BOOTSTRAP-v1";

/// Identity rotation broadcast event prefix (spec §12.2).
pub const ROTATE_V1: &[u8] = b"SHAMIR-ROTATE-v1";

/// Identity rotation orphan-recovery proof prefix (spec §6.5).
pub const ROTATE_PROOF_V1: &[u8] = b"SHAMIR-ROTATE-PROOF-v1";

/// TLS exporter label per RFC 9266 (spec §4.2).
pub const TLS_EXPORTER_LABEL: &[u8] = b"EXPORTER-ShamirDB-AUTH-v1";

/// Resumption ticket AAD prefix (SESSION_RESUMPTION §2.2).
pub const TICKET_V1: &[u8] = b"SHAMIR-TICKET-v1";
