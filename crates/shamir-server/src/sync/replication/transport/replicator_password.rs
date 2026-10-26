//! Replication credential secret type.

use std::fmt;

use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

/// Redacted, zeroizing password retained by follower configuration.
#[derive(Clone)]
pub struct ReplicatorPassword(Zeroizing<String>);

impl ReplicatorPassword {
    /// Build a secret from an owned configuration string.
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// Expose cleartext only at the point where the connection consumes it.
    pub fn reveal(&self) -> &str {
        self.0.as_str()
    }
}

impl From<String> for ReplicatorPassword {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for ReplicatorPassword {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned())
    }
}

impl<'de> Deserialize<'de> for ReplicatorPassword {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

impl fmt::Debug for ReplicatorPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReplicatorPassword(***)")
    }
}
