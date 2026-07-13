//! Crypto primitive wrappers for spec compliance.
//!
//! Thin layer over `argon2`, `hmac`, `hkdf`, `sha2`, `ed25519-dalek`,
//! `aes-gcm`, `subtle`, and `rand` crates that:
//!
//! - Pins parameter choices to the spec (RFC 9106 v1.3 Argon2id, SHA-256 for
//!   HMAC/HKDF, RFC 8032 strict Ed25519).
//! - Enforces zeroization on key material (`Zeroizing<[u8; 32]>`).
//! - Provides `subtle::ConstantTimeEq` wrappers for SCRAM proof comparison.
//! - Returns [`crate::common::Error`] (no upstream errors leak).

use crate::common::error::{Error, Result};
use crate::common::kdf_params::KdfParams;
use ::hkdf::Hkdf;
use aes_gcm::aead::{Aead, AeadInPlace, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use argon2::{Algorithm, Argon2, Params, Version};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hmac::{Hmac, Mac};
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

// ----------------------------------------------------------------------------
// Key newtypes (zeroize on drop)
// ----------------------------------------------------------------------------

/// `salted_password` — output of Argon2id over (password, salt, params).
/// 32 bytes per spec §3.3 / §5.1.3.
pub type SaltedPassword = Zeroizing<[u8; 32]>;

/// `client_key` = HMAC-SHA256(salted_password, "Client Key") — 32 bytes.
pub type ClientKey = Zeroizing<[u8; 32]>;

/// `server_key` = HMAC-SHA256(salted_password, "Server Key") — 32 bytes.
pub type ServerKey = Zeroizing<[u8; 32]>;

/// `stored_key` = SHA256(client_key) — 32 bytes. Stored on the server,
/// **not** zeroized: it is what server holds in `__system__/users`.
///
/// Custom [`Debug`] impl prints `<REDACTED:32>` instead of the bytes
/// (spec IMPL §4 NORMATIVE log redaction).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StoredKey(pub [u8; 32]);

impl core::fmt::Debug for StoredKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("StoredKey(<REDACTED:32>)")
    }
}

/// HMAC tag — 32 bytes (SHA-256 output).
pub type HmacTag = [u8; 32];

// ----------------------------------------------------------------------------
// CSPRNG
// ----------------------------------------------------------------------------

/// Fill `out` with cryptographically secure random bytes.
///
/// Uses [`rand::rngs::OsRng`] — same source as Ed25519 key generation and
/// nonce derivation throughout the spec.
pub fn random_bytes(out: &mut [u8]) {
    rand::rngs::OsRng
        .try_fill_bytes(out)
        .expect("OS RNG failure");
}

/// Generate `N` random bytes as a fresh array.
pub fn random_array<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    random_bytes(&mut out);
    out
}

// ----------------------------------------------------------------------------
// SHA-256
// ----------------------------------------------------------------------------

/// SHA-256 of input bytes.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

// ----------------------------------------------------------------------------
// HMAC-SHA256
// ----------------------------------------------------------------------------

/// HMAC-SHA256(key, data) → 32-byte tag.
///
/// **Design note: per-user Hmac caching is intentionally NOT done.** A
/// natural-looking optimization is to pre-compute `Hmac<Sha256>` instances
/// for `stored_key` / `server_key` in `UserRecord` and clone-then-update on
/// every SCRAM verify (saves ~200-400 ns of ipad/opad init per HMAC).
/// However this would introduce a real-vs-fake user timing channel:
/// real-user path (cached) ~150 ns/HMAC, fake-user path (fresh init)
/// ~300 ns/HMAC. The §8.5 latency padding (50-75 ms) would mask the
/// difference on the wire, but spec §5.2.4 + §9.2 mandate constant-time
/// discipline AS WELL AS padding (defense-in-depth). The ~300 ns
/// savings is dwarfed by Argon2id (~2 s) and by the padding floor (50 ms),
/// so we accept the small CPU cost in exchange for branch-equivalence
/// between real and fake paths.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> HmacTag {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Constant-time equality on byte slices (`subtle::ConstantTimeEq`).
///
/// Use for any comparison of secret-derived values: SCRAM proofs, HMAC tags,
/// pin hashes, bootstrap token hashes, recipient_session_id, etc.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

// ----------------------------------------------------------------------------
// HKDF-SHA256
// ----------------------------------------------------------------------------

/// HKDF-SHA256 Extract+Expand. Output length is bounded by 255×32 = 8160 bytes.
pub fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) -> Result<()> {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    hk.expand(info, out)
        .map_err(|_| Error::Crypto("HKDF expand: invalid length"))?;
    Ok(())
}

// ----------------------------------------------------------------------------
// Argon2id
// ----------------------------------------------------------------------------

/// Argon2id derivation per spec §3.3 / §5.1.3.
///
/// Output: 32 bytes wrapped in [`Zeroizing`] (`SaltedPassword`).
pub fn argon2id(password: &[u8], salt: &[u8], params: &KdfParams) -> Result<SaltedPassword> {
    if params.argon2_version != 0x13 {
        return Err(Error::Crypto("argon2id: unsupported version"));
    }
    let argon_params = Params::new(params.memory_kb, params.time, params.parallelism, Some(32))
        .map_err(|_| Error::Crypto("argon2id: invalid params"))?;

    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password, salt, out.as_mut())
        .map_err(|_| Error::Crypto("argon2id: hash failed"))?;
    Ok(out)
}

// ----------------------------------------------------------------------------
// Ed25519
// ----------------------------------------------------------------------------

/// Ed25519 keypair newtype.
///
/// Wraps [`SigningKey`] which already enables zeroization via the `zeroize`
/// feature in `Cargo.toml`.
///
/// Custom [`Debug`] impl prints only the public key fingerprint and
/// `<REDACTED>` for the private half (spec IMPL §4 NORMATIVE).
pub struct Ed25519Keypair {
    signing: SigningKey,
}

impl Clone for Ed25519Keypair {
    fn clone(&self) -> Self {
        Self {
            signing: self.signing.clone(),
        }
    }
}

impl core::fmt::Debug for Ed25519Keypair {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let pub_bytes = self.signing.verifying_key().to_bytes();
        f.debug_struct("Ed25519Keypair")
            .field("public_pkfp_b64", &base64_first8(&pub_bytes))
            .field("private", &"<REDACTED:32>")
            .finish()
    }
}

fn base64_first8(bytes: &[u8]) -> String {
    // Short identifier — 8 hex chars of the first 4 bytes; enough to tell
    // two keys apart in logs without revealing the public key.
    let n = bytes.len().min(4);
    let mut s = String::with_capacity(2 * n);
    for &b in &bytes[..n] {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

impl Ed25519Keypair {
    /// Generate a fresh keypair from OsRng.
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut seed)
            .expect("OS RNG failure");
        Self {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    /// Reconstruct from a 32-byte seed (only for known-good inputs, e.g. test
    /// vectors and persisted server identity).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// Public key as 32 raw bytes (RFC 8032 §5.1.5 compressed Y).
    /// This is the form hashed in `SHA256(server_pub_key)` everywhere.
    pub fn public_bytes(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    /// Sign `msg`, returning a 64-byte Ed25519 signature.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.signing.sign(msg).to_bytes()
    }
}

/// Ed25519 strict verify (RFC 8032 §6 + small-subgroup rejection).
///
/// Returns `true` iff signature is valid AND public key is canonical AND
/// non-small-order. Per spec §5.5: MUST use `verify_strict` not `verify`.
pub fn ed25519_verify_strict(public_key: &[u8; 32], msg: &[u8], signature: &[u8; 64]) -> bool {
    let Ok(pk) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let sig = Signature::from_bytes(signature);
    pk.verify_strict(msg, &sig).is_ok()
}

// ----------------------------------------------------------------------------
// AES-256-GCM (resumption ticket encryption)
// ----------------------------------------------------------------------------

/// Encrypt `plaintext` with AES-256-GCM. Returns `ciphertext || tag(16)`.
///
/// `nonce` MUST be unique per (key, message) — caller responsible for CSPRNG
/// generation per ticket.
pub fn aes256gcm_encrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::Crypto("AES-GCM: bad key"))?;
    let nonce = Nonce::from_slice(nonce);
    cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| Error::Crypto("AES-GCM: encrypt failed"))
}

/// Decrypt AES-256-GCM. Input is `ciphertext || tag(16)`.
///
/// Returns plaintext on success; [`Error::Crypto`] on tag mismatch (which
/// includes any tampering with `aad`).
pub fn aes256gcm_decrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    ciphertext_and_tag: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::Crypto("AES-GCM: bad key"))?;
    let nonce = Nonce::from_slice(nonce);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext_and_tag,
                aad,
            },
        )
        .map_err(|_| Error::Crypto("AES-GCM: decrypt failed (tag mismatch?)"))
}

/// Build a pre-scheduled AES-256-GCM cipher from a raw 32-byte key.
///
/// **Optim #3:** the AES key schedule (~14 round-keys × 16 bytes) is
/// computed once here. Hot-path callers (e.g. resumption-ticket encrypt /
/// decrypt) MUST cache the resulting cipher and feed it to the `_with_cipher`
/// variants below instead of calling `aes256gcm_encrypt`/`decrypt` which
/// would re-schedule on every call.
pub fn aes256gcm_cipher(key: &[u8; 32]) -> Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(key).map_err(|_| Error::Crypto("AES-GCM: bad key"))
}

/// Encrypt with a pre-scheduled cipher (Optim #3).
pub fn aes256gcm_encrypt_with_cipher(
    cipher: &Aes256Gcm,
    nonce: &[u8; 12],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let nonce = Nonce::from_slice(nonce);
    cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| Error::Crypto("AES-GCM: encrypt failed"))
}

/// Decrypt with a pre-scheduled cipher (Optim #3).
pub fn aes256gcm_decrypt_with_cipher(
    cipher: &Aes256Gcm,
    nonce: &[u8; 12],
    ciphertext_and_tag: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let nonce = Nonce::from_slice(nonce);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext_and_tag,
                aad,
            },
        )
        .map_err(|_| Error::Crypto("AES-GCM: decrypt failed (tag mismatch?)"))
}

/// **Optim #8**: in-place AES-256-GCM decrypt with separate tag.
///
/// `buffer` arrives holding ciphertext and is overwritten with plaintext on
/// success (length unchanged: AES-GCM is a stream cipher under the hood).
/// `tag` is the 16-byte authentication tag carried alongside ciphertext on
/// the wire (e.g. from `TicketWire.tag`).
///
/// Saves the per-call allocation that the owning [`aes256gcm_decrypt_with_cipher`]
/// API forces (it concatenates ciphertext + tag into a fresh `Vec`).
///
/// On tag-mismatch, `buffer` may have been partially overwritten — caller
/// MUST treat its contents as garbage and retry with another cipher key
/// (or fail).
pub fn aes256gcm_decrypt_in_place_with_cipher(
    cipher: &Aes256Gcm,
    nonce: &[u8; 12],
    aad: &[u8],
    buffer: &mut [u8],
    tag: &[u8; 16],
) -> Result<()> {
    let nonce = Nonce::from_slice(nonce);
    let tag = Tag::from_slice(tag);
    cipher
        .decrypt_in_place_detached(nonce, aad, buffer, tag)
        .map_err(|_| Error::Crypto("AES-GCM: decrypt failed (tag mismatch?)"))
}

// Re-export Aes256Gcm so callers don't need to add aes-gcm to their deps.
pub use aes_gcm::Aes256Gcm as Aes256GcmCipher;
