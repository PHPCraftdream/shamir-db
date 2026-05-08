//! Username normalization (spec §15.3).
//!
//! Implements RFC 8265 PRECIS UsernameCaseMapped profile, simplified:
//! - NFC normalization (`unicode-normalization`)
//! - Case-folded (lowercase) for homograph resistance
//! - Reject control chars (Cc), bidi-format chars (Cf) outside allow-list,
//!   private-use plane
//! - Length cap: 255 bytes after normalization
//!
//! Pinned to **Unicode 15.1** for v1 — see spec §15.3 cross-language consistency
//! requirement. JS reference: `new TextEncoder().encode(s.normalize("NFC").toLowerCase()).byteLength`.

use crate::common::error::{Error, Result};
use crate::common::types::limits;
use unicode_normalization::UnicodeNormalization;

/// Canonical normalized username — newtype to prevent accidental mixing
/// with raw `String` from network input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NormalizedUsername(String);

impl NormalizedUsername {
    /// Apply RFC 8265 UsernameCaseMapped normalization.
    ///
    /// Returns [`Error::InvalidUsername`] for: empty, length > 255 bytes after NFC,
    /// or any forbidden character category.
    pub fn from_raw(raw: &str) -> Result<Self> {
        // Step 1: NFC normalize, then lowercase (case-fold).
        // We use simple ASCII-style lowercase; for full PRECIS the lowercase
        // map should be Unicode-aware (e.g. `i` → `i` not Turkish `İ` → `i`).
        // For v1 we accept this simplification but document via test vectors.
        let nfc: String = raw.nfc().collect();
        let folded: String = nfc.to_lowercase().nfc().collect();

        // Step 2: forbidden character classes (PRECIS §9 / RFC 8264 IdentifierClass).
        for ch in folded.chars() {
            if is_forbidden(ch) {
                return Err(Error::InvalidUsername("forbidden character"));
            }
        }

        // Step 3: empty / whitespace-only.
        if folded.is_empty() || folded.chars().all(char::is_whitespace) {
            return Err(Error::InvalidUsername("empty or whitespace-only"));
        }

        // Step 4: length cap.
        if folded.len() > limits::USERNAME_MAX_BYTES {
            return Err(Error::InvalidUsername("> 255 bytes after NFC"));
        }

        Ok(Self(folded))
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

/// Forbidden character predicate (PRECIS §9).
///
/// Rejects:
/// - Control characters (category Cc)
/// - Format characters (category Cf) — strict mode, no allow-list in v1
/// - Private-use characters (Co)
/// - Surrogates (Cs) — already invalid in valid Rust strings, but defensive
/// - Unassigned (Cn) — left to library; we accept for forward compat
fn is_forbidden(ch: char) -> bool {
    use unicode_normalization::char::is_combining_mark;
    let _ = is_combining_mark; // not currently used; reserved for tighter PRECIS

    // Fast path: ASCII control chars.
    if ch.is_control() {
        return true;
    }

    // Codepoint-based class checks. Rust std doesn't expose Unicode general
    // categories directly; we apply targeted bans here. The full PRECIS impl
    // belongs in `precis-profiles` crate (acknowledged TODO in spec deps).
    let cp = ch as u32;

    // Bidi format chars (Cf — explicit allow-list empty in v1).
    if matches!(cp,
        0x200C | 0x200D | 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069 | 0xFEFF
    ) {
        return true;
    }

    // Private-use planes.
    if matches!(cp,
        0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD
    ) {
        return true;
    }

    false
}
