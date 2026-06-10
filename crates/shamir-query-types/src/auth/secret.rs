//! Secret-bearing wrapper for sensitive string fields (passwords, key material).
//!
//! [`SecretString`] wraps a `String` with:
//! - a manual `Debug` that prints `"SecretString(***)"` (never the value),
//! - `Serialize`/`Deserialize` that pass through the inner value unchanged
//!   (so the wire JSON shape stays `"password": "..."`),
//! - `Drop` that zeroizes the heap buffer before freeing it.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(feature = "crypto")]
use zeroize::Zeroize;

/// A `String` whose `Debug` output is redacted and whose contents are
/// zeroized on drop.
///
/// Use this for any field that carries a cleartext secret (password,
/// shared secret) that travels on the wire as a plain JSON/string value
/// but must not leak through `{:?}` / `tracing` / log output.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString {
    inner: String,
}

impl SecretString {
    /// Wrap a plain `String` — the value is taken as-is (not copied).
    pub fn new(s: String) -> Self {
        Self { inner: s }
    }

    /// Reveal the secret cleartext. Use sparingly — only at the
    /// point where the value is consumed (e.g. key derivation).
    pub fn reveal(&self) -> &str {
        &self.inner
    }

    /// Convert into the inner `String`. The caller takes responsibility
    /// for zeroizing it when done.
    pub fn into_inner(mut self) -> String {
        // Prevent our Drop from zeroizing what the caller now owns.
        std::mem::take(&mut self.inner)
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

// ── Serde: transparent pass-through (wire shape unchanged) ──────────

impl Serialize for SecretString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.inner.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self { inner: s })
    }
}

#[cfg(feature = "crypto")]
impl Drop for SecretString {
    fn drop(&mut self) {
        // Zeroize the heap buffer in-place before deallocation.
        // Safety: we are in `drop`, so no other references exist.
        let bytes = unsafe { self.inner.as_bytes_mut() };
        bytes.zeroize();
    }
}

// ── Conversion helpers ──────────────────────────────────────────────

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for SecretString {
    fn from(s: &str) -> Self {
        Self::new(s.to_owned())
    }
}
