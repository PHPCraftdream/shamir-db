//! Secret-bearing wrapper for sensitive string fields (passwords, key material).
//!
//! [`SecretString`] wraps a `String` with:
//! - a manual `Debug` that prints `"SecretString(***)"` (never the value),
//! - `Serialize`/`Deserialize` that pass through the inner value unchanged
//!   (so the wire encoding keeps `"password"` as a plain string),
//! - `Drop` that zeroizes the heap buffer before freeing it with the `crypto` feature.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(feature = "crypto")]
use zeroize::Zeroize;

/// A `String` whose `Debug` output is redacted and whose contents are
/// zeroized on drop when the `crypto` feature is enabled.
///
/// Use this for any field that carries a cleartext secret (password,
/// shared secret) that travels on the wire as a plain string value
/// but must not leak through `{:?}` / `tracing` / log output.
/// Equality is not constant-time and must not be used for authentication.
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

#[cfg(all(test, feature = "crypto"))]
thread_local! {
    static WIPE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LAST_WIPED_LEN: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

#[cfg(feature = "crypto")]
fn wipe(inner: &mut String) {
    #[cfg(all(test, feature = "crypto"))]
    WIPE_CALLS.with(|calls| calls.set(calls.get() + 1));
    #[cfg(all(test, feature = "crypto"))]
    LAST_WIPED_LEN.with(|len| len.set(Some(inner.len())));
    inner.zeroize();
}

#[cfg(all(test, feature = "crypto"))]
pub(crate) fn wipe_calls() -> usize {
    WIPE_CALLS.with(std::cell::Cell::get)
}

#[cfg(all(test, feature = "crypto"))]
pub(crate) fn last_wiped_len() -> Option<usize> {
    LAST_WIPED_LEN.with(std::cell::Cell::get)
}

#[cfg(all(test, feature = "crypto"))]
#[path = "tests/secret_wipe_tests.rs"]
mod secret_wipe_tests;

#[cfg(feature = "crypto")]
impl Drop for SecretString {
    fn drop(&mut self) {
        wipe(&mut self.inner);
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
