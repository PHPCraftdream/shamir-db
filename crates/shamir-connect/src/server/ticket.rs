//! Server-side resumption ticket: AES-256-GCM encrypted self-contained state.
//!
//! Per SESSION_RESUMPTION §2 the ticket carries the per-(user, ticket_family_id)
//! state needed to recreate a session without re-running SCRAM. Wire layout:
//!
//! ```text
//! ticket_wire = struct {
//!   version: u8 = 1,
//!   nonce: bytes(12),
//!   ciphertext_len: u16_be,
//!   ciphertext: bytes,
//!   tag: bytes(16),
//! }
//!
//! aad = "SHAMIR-TICKET-v1" || u8(version)
//! ```
//!
//! AAD is **only** envelope-visible bytes (version + domain tag). The
//! `transport_kind_at_auth` and `binding_mode_at_auth` fields live inside
//! `ticket_plain` — GCM tag covers all of plaintext, so tampering with any
//! field flips tag verification.

use crate::common::crypto::{aes256gcm_decrypt, aes256gcm_encrypt, random_array};
use crate::common::domain_tags::TICKET_V1;
use crate::common::error::{Error, Result};
use crate::common::types::{BindingMode, TransportKind};
use serde::{Deserialize, Serialize};

/// Plaintext fields of the ticket. Encoded as msgpack (canonical form
/// by msgpack-rs default — sufficient for v1 since AAD does not depend on
/// any inner field).
///
/// Per SESSION_RESUMPTION §2.1 / diagram 02 step 12, `roles` is the
/// permissions snapshot taken at full SCRAM time; resumed sessions MUST be
/// constructed with these roles so admin sessions retain admin powers.
///
/// **Optim #2:** fixed-size byte fields (`user_id`, `channel_binding_at_auth`,
/// `ticket_family_id`) use [`serde_bytes::ByteArray<N>`] instead of
/// `Vec<u8>` — eliminates per-resume heap allocation for these fields and
/// removes the `parse_user_id`/`parse_family_id` length-check helpers.
/// Wire format identical (msgpack `bin` of length N).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketPlain {
    /// Plaintext version byte (must equal envelope.version).
    pub version: u8,
    /// User identifier (16 bytes, e.g. UUID).
    pub user_id: serde_bytes::ByteArray<16>,
    /// Username post-NFC (UTF-8).
    pub username_nfc: String,
    /// `transport_kind` at time of full SCRAM auth.
    pub transport_kind_at_auth: u8,
    /// `binding_mode` at time of full SCRAM auth.
    pub binding_mode_at_auth: u8,
    /// `tls_exporter_or_zeros` at time of full SCRAM auth (32 bytes).
    pub channel_binding_at_auth: serde_bytes::ByteArray<32>,
    /// 16-byte ticket family id (per-device lineage). Counter is per-(user, family).
    pub ticket_family_id: serde_bytes::ByteArray<16>,
    /// Original full-SCRAM time — does NOT update on refreshTicket.
    pub original_auth_at_ns: u64,
    /// Absolute ticket expiry.
    pub expires_at_ns: u64,
    /// Monotonic counter within the family.
    pub family_counter: u64,
    /// Permissions snapshot at full SCRAM time (SESSION_RESUMPTION §2.1).
    /// Resume rebuilds [`SessionPermissions`] from these so e.g. a `superuser`
    /// session resumed via ticket retains admin authorization.
    pub roles: Vec<String>,
    /// Identity-key version: which Ed25519 keypair was current when this
    /// ticket was issued. Allows server to reject tickets issued before a
    /// rotation overlap window (spec §5.7 NORMATIVE / diagram 12).
    /// Increments on every `rotateServerIdentity`. `0` for first-ever key.
    pub identity_key_version: u64,
}

/// Wire envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketWire {
    /// Visible version byte (used for AAD construction).
    pub version: u8,
    /// 12-byte AES-GCM nonce.
    pub nonce: [u8; 12],
    /// Ciphertext (excluding tag).
    pub ciphertext: Vec<u8>,
    /// 16-byte AES-GCM tag.
    pub tag: [u8; 16],
}

impl TicketWire {
    /// Serialize to flat byte string `version || nonce || u16_be(ciphertext_len)
    /// || ciphertext || tag`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 12 + 2 + self.ciphertext.len() + 16);
        out.push(self.version);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&(self.ciphertext.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.ciphertext);
        out.extend_from_slice(&self.tag);
        out
    }

    /// Parse from bytes.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < 1 + 12 + 2 + 16 {
            return Err(Error::InvalidInput("ticket_wire: too short"));
        }
        let version = buf[0];
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&buf[1..13]);
        let ct_len = u16::from_be_bytes([buf[13], buf[14]]) as usize;
        let expected_total = 1 + 12 + 2 + ct_len + 16;
        if buf.len() != expected_total {
            return Err(Error::InvalidInput("ticket_wire: length mismatch"));
        }
        let ciphertext = buf[15..15 + ct_len].to_vec();
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&buf[15 + ct_len..]);
        Ok(Self {
            version,
            nonce,
            ciphertext,
            tag,
        })
    }
}

/// Build AAD: `"SHAMIR-TICKET-v1" || u8(version)` (envelope-visible only).
fn build_aad(version: u8) -> Vec<u8> {
    let mut aad = Vec::with_capacity(TICKET_V1.len() + 1);
    aad.extend_from_slice(TICKET_V1);
    aad.push(version);
    aad
}

/// Encrypt a [`TicketPlain`] with `ticket_key` → wire envelope.
pub fn encrypt_ticket(ticket_key: &[u8; 32], plain: &TicketPlain) -> Result<TicketWire> {
    let plaintext = rmp_serde::to_vec_named(plain)
        .map_err(|e| Error::Encoding(format!("ticket msgpack: {e}")))?;

    let nonce = random_array::<12>();
    let aad = build_aad(plain.version);

    let ct_with_tag = aes256gcm_encrypt(ticket_key, &nonce, &plaintext, &aad)?;
    if ct_with_tag.len() < 16 {
        return Err(Error::Crypto("AES-GCM: short output"));
    }
    let split = ct_with_tag.len() - 16;
    let ciphertext = ct_with_tag[..split].to_vec();
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&ct_with_tag[split..]);

    Ok(TicketWire {
        version: plain.version,
        nonce,
        ciphertext,
        tag,
    })
}

/// Decrypt a [`TicketWire`] using `current_key` first, falling back to
/// `previous_key` (during 24h rotation overlap, SESSION_RESUMPTION §3.2).
///
/// Returns the parsed `TicketPlain` after AAD validation. The caller is then
/// responsible for the remaining checks (expiry, downgrade, family counter
/// CAS) in spec §5.4.
pub fn decrypt_ticket(
    current_key: &[u8; 32],
    previous_key: Option<&[u8; 32]>,
    wire: &TicketWire,
) -> Result<TicketPlain> {
    if wire.version != 1 {
        return Err(Error::InvalidInput("ticket: unsupported version"));
    }
    let aad = build_aad(wire.version);

    // Reassemble ciphertext || tag for the aes-gcm crate.
    let mut ct_with_tag = Vec::with_capacity(wire.ciphertext.len() + 16);
    ct_with_tag.extend_from_slice(&wire.ciphertext);
    ct_with_tag.extend_from_slice(&wire.tag);

    let plaintext = match aes256gcm_decrypt(current_key, &wire.nonce, &ct_with_tag, &aad) {
        Ok(pt) => pt,
        Err(_) => match previous_key {
            Some(prev) => aes256gcm_decrypt(prev, &wire.nonce, &ct_with_tag, &aad)?,
            None => return Err(Error::Crypto("AES-GCM: decrypt failed")),
        },
    };

    let plain: TicketPlain = rmp_serde::from_slice(&plaintext)
        .map_err(|e| Error::Encoding(format!("ticket parse: {e}")))?;

    // Defense-in-depth: plaintext.version MUST match envelope.version.
    if plain.version != wire.version {
        return Err(Error::InvalidInput("ticket: version mismatch"));
    }

    Ok(plain)
}

/// Helper: validate `transport_kind_at_auth` and `binding_mode_at_auth`
/// fields parse to known enum values (fail-closed per spec §4.2).
pub fn validate_ticket_enums(plain: &TicketPlain) -> Result<(TransportKind, BindingMode)> {
    let tk = TransportKind::from_u8(plain.transport_kind_at_auth)?;
    let bm = BindingMode::from_u8(plain.binding_mode_at_auth)?;
    Ok((tk, bm))
}

/// Anti-downgrade check (SESSION_RESUMPTION §6.1). Resume rejected if
/// `binding_strength(now) < binding_strength(at_auth)`.
///
/// `allow_browser_upgrade = false` → also reject `0x02 → 0x01` (browser to
/// native), per `[strict] allow_browser_ticket_upgrade = false`.
pub fn check_anti_downgrade(
    binding_mode_at_auth: BindingMode,
    binding_mode_now: BindingMode,
    allow_browser_upgrade: bool,
) -> Result<()> {
    if binding_mode_now.strength() < binding_mode_at_auth.strength() {
        return Err(Error::AuthFailed); // generic per spec
    }
    if !allow_browser_upgrade
        && binding_mode_at_auth == BindingMode::TlsNoExport
        && binding_mode_now == BindingMode::TlsExporter
    {
        return Err(Error::AuthFailed);
    }
    Ok(())
}

/// Defaults for ticket-related limits (SESSION_RESUMPTION §6.6).
pub mod ticket_limits {
    use crate::common::time::ns;

    /// `RESUMPTION_TTL`: 1 hour from issue (per-ticket).
    pub const RESUMPTION_TTL_NS: u64 = ns::HOUR;
    /// `RESUMPTION_MAX_CHAIN_AGE` = `SESSION_MAX_AGE` = 24 hours.
    pub const RESUMPTION_MAX_CHAIN_AGE_NS: u64 = 24 * ns::HOUR;
    /// `TICKET_KEY_ROTATION` cadence.
    pub const TICKET_KEY_ROTATION_NS: u64 = 24 * ns::HOUR;
}
