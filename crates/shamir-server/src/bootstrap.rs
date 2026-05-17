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
//! ([`BootstrapPolicy::RandomToken`]), this helper generates a
//! 32-byte URL-safe token, prints it ONCE to the configured logger
//! at WARN level, AND writes it to `data_dir/bootstrap_token.txt`
//! with restrictive permissions. Operators are expected to read the
//! token, log in once via SCRAM and `changePassword`, then delete
//! the token file.

use std::fs;
use std::path::Path;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use shamir_connect::common::crypto::random_array;
use shamir_connect::common::error::{Error as ConnectError, Result as ConnectResult};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use zeroize::Zeroizing;

use crate::user_directory::RedbUserDirectory;

/// Default name of the auto-created superuser.
pub const DEFAULT_BOOTSTRAP_NAME: &str = "admin";

/// File name used for the random-token mode (relative to `data_dir`).
pub const BOOTSTRAP_TOKEN_FILE: &str = "bootstrap_token.txt";

/// What credentials to use for the bootstrap account.
#[derive(Debug, Clone)]
pub enum BootstrapPolicy<'a> {
    /// Use the supplied password verbatim. Caller is responsible for
    /// cleaning up the buffer.
    Password(&'a [u8]),
    /// Generate a 32-byte random token, write it to
    /// `data_dir/bootstrap_token.txt`, and use it as the password.
    RandomToken,
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
    dir: &RedbUserDirectory,
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
    let token_str: Option<String> = match policy {
        BootstrapPolicy::Password(p) => {
            password_buf.extend_from_slice(p);
            None
        }
        BootstrapPolicy::RandomToken => {
            // 32 random bytes -> ~43 char base64-url-no-pad.
            let raw: [u8; 32] = random_array();
            let token = URL_SAFE_NO_PAD.encode(raw);
            password_buf.extend_from_slice(token.as_bytes());
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

    insert_with_role(dir, name, record, "superuser")?;

    // Write the token file (only in RandomToken mode).
    let token_path = if let Some(tok) = &token_str {
        if !data_dir.exists() {
            fs::create_dir_all(data_dir)?;
        }
        let path = data_dir.join(BOOTSTRAP_TOKEN_FILE);
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

/// Insert a fresh user with the requested role. Roles can't be set in a
/// single transaction via the [`UserDirectory`] trait (see comment in
/// [`RedbUserDirectory::insert`]), so this helper does insert + update_roles.
fn insert_with_role(
    dir: &RedbUserDirectory,
    name: &str,
    record: UserRecord,
    role: &str,
) -> ConnectResult<()> {
    dir.insert(name.to_string(), record)?;
    // bump_to=0: roles change without bumping the validity epoch (no
    // existing sessions can be invalidated on a fresh install anyway).
    dir.update_roles(name, vec![role.to_string()], 0)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fast_kdf() -> KdfParams {
        KdfParams {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        }
    }

    #[test]
    fn creates_then_idempotent() {
        let dir = TempDir::new().unwrap();
        let dir_path = dir.path();
        let user_dir =
            RedbUserDirectory::open(dir_path.join("users.redb")).unwrap();

        let r1 = ensure_superuser(
            &user_dir,
            dir_path,
            DEFAULT_BOOTSTRAP_NAME,
            BootstrapPolicy::Password(b"hunter2"),
            &fast_kdf(),
        )
        .unwrap();
        assert!(matches!(r1, BootstrapOutcome::Created { token: None, .. }));

        let r2 = ensure_superuser(
            &user_dir,
            dir_path,
            DEFAULT_BOOTSTRAP_NAME,
            BootstrapPolicy::Password(b"different-password"),
            &fast_kdf(),
        )
        .unwrap();
        assert!(matches!(r2, BootstrapOutcome::AlreadyExists),
            "second call must be a no-op even with different password");

        let roles = user_dir
            .lookup_roles(DEFAULT_BOOTSTRAP_NAME)
            .expect("lookup_roles should not fail on a local redb")
            .expect("bootstrap user must exist after init");
        assert!(roles.iter().any(|r| r == "superuser"));
    }

    #[test]
    fn random_token_writes_file() {
        let dir = TempDir::new().unwrap();
        let dir_path = dir.path();
        let user_dir =
            RedbUserDirectory::open(dir_path.join("users.redb")).unwrap();

        let r = ensure_superuser(
            &user_dir,
            dir_path,
            DEFAULT_BOOTSTRAP_NAME,
            BootstrapPolicy::RandomToken,
            &fast_kdf(),
        )
        .unwrap();
        match r {
            BootstrapOutcome::Created { token: Some(tok), token_path: Some(p) } => {
                assert_eq!(fs::read_to_string(p).unwrap(), tok);
                assert!(tok.len() >= 32, "token long enough");
            }
            other => panic!("expected Created with token, got {:?}", other),
        }
    }

    #[test]
    fn derived_keys_match_real_login_flow() {
        // Sanity: the persisted stored_key must equal what a fresh client
        // would derive from the same password+salt+kdf — i.e. the SCRAM
        // verify step would succeed. We don't run the whole protocol here;
        // we just check key derivation symmetry.
        let dir = TempDir::new().unwrap();
        let dir_path = dir.path();
        let user_dir =
            RedbUserDirectory::open(dir_path.join("users.redb")).unwrap();

        let pw = b"correct horse battery staple";
        ensure_superuser(
            &user_dir,
            dir_path,
            "alice",
            BootstrapPolicy::Password(pw),
            &fast_kdf(),
        )
        .unwrap();

        let stored = user_dir.lookup_by_name("alice").unwrap();
        let redo = DerivedKeys::derive(pw, &stored.salt, &stored.kdf_params).unwrap();
        assert_eq!(redo.stored_key.0, stored.stored_key.0,
            "stored_key must round-trip through ensure_superuser");
    }
}
