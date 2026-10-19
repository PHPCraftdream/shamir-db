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
