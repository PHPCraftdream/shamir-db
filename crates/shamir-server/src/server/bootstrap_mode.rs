//! Boot bootstrap-mode policy variants.

use zeroize::Zeroizing;

/// Bootstrap policy options exposed at boot time.
pub enum BootstrapMode {
    /// Use the supplied password verbatim.
    Password {
        username: String,
        password: Zeroizing<Vec<u8>>,
    },
    /// Generate a 32-byte random token (printed to logs + written to
    /// `data_dir/bootstrap_token.txt`). Username defaults to `admin`.
    RandomToken {
        /// Optional override of the default `admin` username.
        username: Option<String>,
    },
    /// Skip bootstrap entirely; assume the directory already has a
    /// superuser. Used when the operator manages users out-of-band.
    Skip,
}

impl Default for BootstrapMode {
    fn default() -> Self {
        BootstrapMode::RandomToken { username: None }
    }
}
