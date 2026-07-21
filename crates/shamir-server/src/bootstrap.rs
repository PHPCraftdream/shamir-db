//! Bootstrap helper — creates the first `superuser` SCRAM account on a
//! fresh data directory.
//!
//! On clean install the user directory is empty, so no client can log in.
//! [`ensure_superuser`] writes a single SCRAM record (Argon2id-derived
//! `stored_key` + `server_key` from the supplied password, with the role
//! `superuser`) iff the directory currently holds no entry for that name.
//!
//! Idempotent: a subsequent boot with the same password is a no-op. A
//! subsequent boot with a *different* password does **not** rotate the
//! existing record — password rotation is a separate flow (the
//! `changePassword` SCRAM ceremony, spec §12.5). If the operator wants
//! to force-rotate the bootstrap user, they should delete the user
//! directory file (or use a future admin CLI flag).
//!
//! Random-token mode: when the operator passes no password
//! ([`BootstrapPolicy::RandomToken`]), this helper generates a 32-byte
//! URL-safe token, prints it ONCE to the configured logger at WARN level,
//! AND writes it to `data_dir/bootstrap_token.txt` (or the operator's
//! `--bootstrap-token-path` override — see `BootstrapPolicy::RandomToken`)
//! with restrictive permissions. The token now auto-deletes itself: the
//! server (`server_launcher.rs`, wired via `ServerMetaStore`) removes the
//! file and consumes the token record on the FIRST successful login for
//! this username (primary path), or via a 24h TTL boot-time sweep for any
//! token nobody ever used (backstop). Operators who want an immediate
//! belt-and-braces guarantee can still delete the file manually right
//! after reading it and logging in — that remains safe and recommended,
//! it's just no longer the only mechanism.

use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use shamir_connect::common::crypto::random_array;
use shamir_connect::common::error::{Error as ConnectError, Result as ConnectResult};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use zeroize::Zeroizing;

use crate::user_directory::FjallUserDirectory;

/// Default name of the auto-created superuser.
pub const DEFAULT_BOOTSTRAP_NAME: &str = "admin";

/// File name used for the random-token mode (relative to `data_dir`).
pub const BOOTSTRAP_TOKEN_FILE: &str = "bootstrap_token.txt";

/// TTL for an outstanding bootstrap token, matching the existing
/// resumption-ticket TTL convention (`handshake.rs::RESUMPTION_TICKET_TTL_NS`).
/// A token nobody ever used to log in is best-effort-deleted and consumed
/// by the boot-time sweep in `server_launcher.rs` once this TTL elapses.
pub const BOOTSTRAP_TOKEN_TTL_NS: u64 = 24 * shamir_connect::common::time::ns::HOUR;

/// What credentials to use for the bootstrap account.
#[derive(Debug, Clone)]
pub enum BootstrapPolicy<'a> {
    /// Use the supplied password verbatim. Caller is responsible for
    /// cleaning up the buffer.
    Password(&'a [u8]),
    /// Generate a 32-byte random token and use it as the password. The
    /// optional path overrides the default `data_dir/bootstrap_token.txt`
    /// output location (recommended: a tmpfs path so `backup --to` never
    /// captures it).
    RandomToken(Option<PathBuf>),
}

/// Outcome of [`ensure_superuser`].
#[derive(Debug, Clone)]
pub enum BootstrapOutcome {
    /// The directory already contained an entry for `name`; nothing was
    /// touched.
    AlreadyExists,
    /// A new entry was created. For [`BootstrapPolicy::RandomToken`] mode
    /// the token was also written to disk; the path is returned.
    Created {
        /// `Some(token)` only for `RandomToken` mode; `None` if the
        /// caller supplied a password.
        token: Option<String>,
        /// Path to the token file (when present), for the operator to
        /// `cat` / `tail` and then delete.
        token_path: Option<std::path::PathBuf>,
    },
}

/// Errors specific to the bootstrap helper.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// Argon2id derivation, redb commit, or other shamir-connect failure.
    #[error("bootstrap: {0}")]
    Connect(#[from] ConnectError),
    /// File-system error writing the token file.
    #[error("bootstrap io: {0}")]
    Io(#[from] std::io::Error),
}

/// Make sure a superuser exists in `dir`. See module docs for semantics.
///
/// `data_dir` is only used in `RandomToken` mode (to place the token
/// file). The `kdf` params control the Argon2id cost — pass the same
/// defaults the production handshake will use so a token-mode account
/// can log in straight away.
pub fn ensure_superuser(
    dir: &FjallUserDirectory,
    data_dir: &Path,
    name: &str,
    policy: BootstrapPolicy<'_>,
    kdf: &KdfParams,
) -> Result<BootstrapOutcome, BootstrapError> {
    if dir.lookup_by_name(name).is_some() {
        return Ok(BootstrapOutcome::AlreadyExists);
    }

    // Materialise the password — either supplied or generated.
    let mut password_buf: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::new());
    let mut token_path_override: Option<PathBuf> = None;
    let token_str: Option<String> = match policy {
        BootstrapPolicy::Password(p) => {
            password_buf.extend_from_slice(p);
            None
        }
        BootstrapPolicy::RandomToken(override_path) => {
            // 32 random bytes -> ~43 char base64-url-no-pad.
            let raw: [u8; 32] = random_array();
            let token = URL_SAFE_NO_PAD.encode(raw);
            password_buf.extend_from_slice(token.as_bytes());
            token_path_override = override_path;
            Some(token)
        }
    };

    // Derive SCRAM keys.
    let salt: [u8; 16] = random_array();
    let derived = DerivedKeys::derive(&password_buf, &salt, kdf)?;

    let mut server_key_z: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
    server_key_z.copy_from_slice(&derived.server_key[..]);

    let record = UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: server_key_z,
        kdf_params: *kdf,
        tickets_invalid_before_ns: 0,
    };

    insert_superuser(dir, name, record)?;

    // Write the token file (only in RandomToken mode).
    let token_path = if let Some(tok) = &token_str {
        let path = token_path_override.unwrap_or_else(|| data_dir.join(BOOTSTRAP_TOKEN_FILE));
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(&path, tok.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o600);
            let _ = fs::set_permissions(&path, perms);
        }
        Some(path)
    } else {
        None
    };

    Ok(BootstrapOutcome::Created {
        token: token_str,
        token_path,
    })
}

/// Insert a fresh user and grant it superuser status. The superuser flag
/// can't be set in a single transaction via the [`UserDirectory`] trait
/// (see comment in [`FjallUserDirectory::insert`]), so this helper does
/// `insert` + `set_superuser`.
///
/// Task #557: previously this helper was `insert_with_role(.., "superuser")`
/// which went through `update_roles` — but task #557 reserves the literal
/// `"superuser"` string at the directory write boundary, so the superuser
/// flag is now flipped via the dedicated `set_superuser` method instead.
fn insert_superuser(dir: &FjallUserDirectory, name: &str, record: UserRecord) -> ConnectResult<()> {
    dir.insert(name.to_string(), record)?;
    // now_ns=0: grant without bumping the validity epoch (no existing
    // sessions can be invalidated on a fresh install anyway).
    dir.set_superuser(name, true, 0)?;
    Ok(())
}
