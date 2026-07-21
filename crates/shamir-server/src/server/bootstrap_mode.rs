//! Boot bootstrap-mode policy variants.

use std::path::PathBuf;

use zeroize::Zeroizing;

/// Bootstrap policy options exposed at boot time.
pub enum BootstrapMode {
    /// Use the supplied password verbatim.
    Password {
        username: String,
        password: Zeroizing<Vec<u8>>,
    },
    /// Generate a 32-byte random token (printed to logs + written to
    /// `data_dir/bootstrap_token.txt`, or the overridden `token_path`).
    /// Username defaults to `admin`. The token auto-deletes on first
    /// successful login or a 24h TTL, whichever comes first.
    RandomToken {
        /// Optional override of the default `admin` username.
        username: Option<String>,
        /// Optional override of the default `data_dir/bootstrap_token.txt`
        /// path — recommended to point at a tmpfs path (e.g.
        /// `/run/shamir/bootstrap_token.txt`) so the token is never swept
        /// into a `backup --to` snapshot of `data_dir`.
        token_path: Option<PathBuf>,
    },
    /// Skip bootstrap entirely; assume the directory already has a
    /// superuser. Used when the operator manages users out-of-band.
    Skip,
}

impl Default for BootstrapMode {
    fn default() -> Self {
        BootstrapMode::RandomToken {
            username: None,
            token_path: None,
        }
    }
}
