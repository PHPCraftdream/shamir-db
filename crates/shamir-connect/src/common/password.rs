//! Client-side password policy enforcement (spec §3.2).
//!
//! ```text
//! PASSWORD_MIN_LENGTH = 12 chars (UTF-8 code points)
//! PASSWORD_MAX_LENGTH = 1024 chars
//! Forbidden: empty, only-whitespace, single-repeated-char
//! ```
//!
//! Server cannot verify policy through SCRAM by design (it never sees the
//! password); validation is the client's responsibility BEFORE running
//! Argon2id. This module is referenced by [`crate::client::bootstrap`] and
//! [`crate::client::changepw`].

use crate::common::error::{Error, Result};
use crate::common::types::limits::{PASSWORD_MAX_CHARS, PASSWORD_MIN_CHARS};

/// Validate `password` against spec §3.2 policy. Returns `Err` with a stable
/// (non-secret) reason string on failure. The password itself is never
/// included in the error.
pub fn validate_password(password: &[u8]) -> Result<()> {
    let s = std::str::from_utf8(password).map_err(|_| Error::InvalidPassword("not valid UTF-8"))?;

    let char_count = s.chars().count();
    if char_count < PASSWORD_MIN_CHARS {
        return Err(Error::InvalidPassword("shorter than PASSWORD_MIN_LENGTH"));
    }
    if char_count > PASSWORD_MAX_CHARS {
        return Err(Error::InvalidPassword("longer than PASSWORD_MAX_LENGTH"));
    }

    // empty already covered by min length, but kept explicit for clarity.
    if s.is_empty() {
        return Err(Error::InvalidPassword("empty"));
    }

    // whitespace-only check.
    if s.chars().all(|c| c.is_whitespace()) {
        return Err(Error::InvalidPassword("only whitespace"));
    }

    // Single-repeated-char check: e.g., "aaaaaaaaaaaa".
    let first = s.chars().next().expect("non-empty checked above");
    if s.chars().all(|c| c == first) {
        return Err(Error::InvalidPassword("single repeated character"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_typical_strong_password() {
        assert!(validate_password(b"correct horse battery staple").is_ok());
    }

    #[test]
    fn rejects_short_password_per_spec_3_2() {
        // 11 chars — one below the 12-char min.
        assert!(matches!(
            validate_password(b"shortpass11"),
            Err(Error::InvalidPassword(_))
        ));
    }

    #[test]
    fn accepts_exactly_min_length() {
        // 12 chars exactly.
        assert!(validate_password(b"twelvechars1").is_ok());
    }

    #[test]
    fn rejects_too_long_password() {
        let long = vec![b'a'; PASSWORD_MAX_CHARS + 1];
        assert!(matches!(
            validate_password(&long),
            Err(Error::InvalidPassword(_))
        ));
    }

    #[test]
    fn rejects_empty_password() {
        assert!(matches!(
            validate_password(b""),
            Err(Error::InvalidPassword(_))
        ));
    }

    #[test]
    fn rejects_whitespace_only_password() {
        assert!(matches!(
            validate_password(b"            "), // 12 spaces
            Err(Error::InvalidPassword(_))
        ));
    }

    #[test]
    fn rejects_single_repeated_char_password() {
        assert!(matches!(
            validate_password(b"aaaaaaaaaaaa"), // 12 'a's
            Err(Error::InvalidPassword(_))
        ));
    }

    #[test]
    fn rejects_invalid_utf8() {
        assert!(matches!(
            validate_password(&[0xff, 0xfe, 0xfd]),
            Err(Error::InvalidPassword(_))
        ));
    }

    #[test]
    fn counts_code_points_not_bytes_for_min_length() {
        // 4 multi-byte chars (each emoji is 4 bytes) — 4 chars < 12 → reject.
        let four_emoji = "🎯🎯🎯🎯";
        assert!(matches!(
            validate_password(four_emoji.as_bytes()),
            Err(Error::InvalidPassword(_))
        ));

        // 12 distinct emoji = 12 chars → accept (and not single-repeated).
        let twelve_distinct = "🎯🦀🌊🎲🍀🚀🎨🎵🎭🎬🎮🎲";
        assert!(validate_password(twelve_distinct.as_bytes()).is_ok());
    }
}
