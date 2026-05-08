//! Username normalization (spec §15.3) — full RFC 8265 PRECIS
//! UsernameCaseMapped profile.
//!
//! v1 #2: backed by the [`precis-profiles`] crate which implements the
//! complete UsernameCaseMapped profile per RFC 8265 §3.3:
//!
//! 1. **Width Mapping** — half-width / full-width forms folded.
//! 2. **Case Mapping** — Unicode case-fold (per RFC 8265, NOT raw
//!    `String::to_lowercase`; e.g. Turkish `İ` casefolds to `i\u{307}`,
//!    German `ß` to `ss`).
//! 3. **NFC Normalization** — applied AFTER case mapping.
//! 4. **Directionality** — reject ambiguous bidi.
//! 5. **Restrictions** — control chars, private-use, non-characters,
//!    incompatible-with-IdentifierClass code points are rejected.
//!
//! This is the spec §15.3 NORMATIVE form. Cross-language consistency is
//! preserved against any other RFC-8265-compliant implementation
//! (JavaScript, Python, etc.) — `to_lowercase()` was a release-blocker
//! gap.
//!
//! Length cap (255 bytes after normalization) is applied on top of the
//! PRECIS profile result.

use crate::common::error::{Error, Result};
use crate::common::types::limits;
use precis_profiles::precis_core::profile::Profile;
use precis_profiles::UsernameCaseMapped;

/// Canonical normalized username — newtype to prevent accidental mixing
/// with raw `String` from network input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NormalizedUsername(String);

impl NormalizedUsername {
    /// Apply RFC 8265 UsernameCaseMapped normalization.
    ///
    /// Returns [`Error::InvalidUsername`] for: empty, length > 255 bytes
    /// after normalization, or any forbidden character category per the
    /// PRECIS profile.
    pub fn from_raw(raw: &str) -> Result<Self> {
        // PRECIS UsernameCaseMapped: width mapping → case mapping (RFC 8265
        // case-fold, not String::to_lowercase) → NFC normalization →
        // directionality + restrictions checks.
        let profile = UsernameCaseMapped::new();
        let mapped = profile
            .enforce(raw)
            .map_err(|_| Error::InvalidUsername("PRECIS UsernameCaseMapped rejected input"))?;
        let mapped_str = mapped.into_owned();

        // Empty or whitespace-only is rejected (PRECIS Empty error class +
        // defensive).
        if mapped_str.is_empty() || mapped_str.chars().all(char::is_whitespace) {
            return Err(Error::InvalidUsername("empty or whitespace-only"));
        }

        // Length cap on top of PRECIS.
        if mapped_str.len() > limits::USERNAME_MAX_BYTES {
            return Err(Error::InvalidUsername("> 255 bytes after PRECIS"));
        }

        Ok(Self(mapped_str))
    }

    /// Underlying UTF-8 bytes (post-normalization) — embedded in `auth_message`.
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Underlying string ref.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Construct from already-normalized string without re-validation.
    ///
    /// **Caller is responsible** for ensuring the input has gone through
    /// [`Self::from_raw`] previously (e.g. when re-loading from SystemStore).
    pub fn from_normalized_unchecked(normalized: String) -> Self {
        Self(normalized)
    }
}
