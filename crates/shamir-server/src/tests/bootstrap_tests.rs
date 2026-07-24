use std::fs;
use std::path::PathBuf;

use shamir_connect::common::crypto::sha256;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::time::UnixNanos;
use tempfile::TempDir;

use crate::bootstrap::{
    ensure_superuser, rotate_bootstrap_credential_to_random, BootstrapOutcome, BootstrapPolicy,
    DEFAULT_BOOTSTRAP_NAME,
};
use crate::server_meta::ServerMetaStore;
use crate::user_directory::FjallUserDirectory;
use shamir_connect::server::admin::UserDirectory;

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
    let user_dir = FjallUserDirectory::open(dir_path.join("users.redb")).unwrap();

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
    assert!(
        matches!(r2, BootstrapOutcome::AlreadyExists),
        "second call must be a no-op even with different password"
    );

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
    let user_dir = FjallUserDirectory::open(dir_path.join("users.redb")).unwrap();

    let r = ensure_superuser(
        &user_dir,
        dir_path,
        DEFAULT_BOOTSTRAP_NAME,
        BootstrapPolicy::RandomToken(None),
        &fast_kdf(),
    )
    .unwrap();
    match r {
        BootstrapOutcome::Created {
            token: Some(tok),
            token_path: Some(p),
        } => {
            assert_eq!(fs::read_to_string(p).unwrap(), tok);
            assert!(tok.len() >= 32, "token long enough");
        }
        other => panic!("expected Created with token, got {:?}", other),
    }
}

#[test]
fn random_token_writes_file_at_override_path() {
    // RI-9: `BootstrapPolicy::RandomToken(Some(override_path))` must write
    // the token file at `override_path`, NOT `data_dir/bootstrap_token.txt`
    // — the whole point of the override is to let operators point the
    // token at a tmpfs path outside `data_dir` (so `backup --to` never
    // captures it).
    let dir = TempDir::new().unwrap();
    let dir_path = dir.path();
    let user_dir = FjallUserDirectory::open(dir_path.join("users.redb")).unwrap();

    let override_dir = TempDir::new().unwrap();
    let override_path = override_dir.path().join("nested").join("token.txt");

    let r = ensure_superuser(
        &user_dir,
        dir_path,
        DEFAULT_BOOTSTRAP_NAME,
        BootstrapPolicy::RandomToken(Some(override_path.clone())),
        &fast_kdf(),
    )
    .unwrap();

    match r {
        BootstrapOutcome::Created {
            token: Some(tok),
            token_path: Some(p),
        } => {
            assert_eq!(
                p, override_path,
                "token must be written at the override path"
            );
            assert_eq!(fs::read_to_string(&p).unwrap(), tok);
        }
        other => panic!("expected Created with token, got {:?}", other),
    }

    let default_path = dir_path.join(crate::bootstrap::BOOTSTRAP_TOKEN_FILE);
    assert!(
        !default_path.exists(),
        "default data_dir token path must NOT be written when an override is given"
    );
}

#[test]
fn derived_keys_match_real_login_flow() {
    // Sanity: the persisted stored_key must equal what a fresh client
    // would derive from the same password+salt+kdf — i.e. the SCRAM
    // verify step would succeed. We don't run the whole protocol here;
    // we just check key derivation symmetry.
    let dir = TempDir::new().unwrap();
    let dir_path = dir.path();
    let user_dir = FjallUserDirectory::open(dir_path.join("users.redb")).unwrap();

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
    assert_eq!(
        redo.stored_key.0, stored.stored_key.0,
        "stored_key must round-trip through ensure_superuser"
    );
}

/// CR-A6 unit coverage: [`rotate_bootstrap_credential_to_random`] must
/// replace BOTH the salt and the `stored_key` with fresh values, so the
/// original bootstrap token (used as the password here) no longer verifies
/// against the post-rotation record.
#[tokio::test]
async fn rotate_bootstrap_credential_invalidates_old_password() {
    let dir = TempDir::new().unwrap();
    let dir_path = dir.path();
    let user_dir = FjallUserDirectory::open(dir_path.join("users.redb")).unwrap();
    let kdf = fast_kdf();

    let token = "the-bootstrap-token-used-as-a-password";
    ensure_superuser(
        &user_dir,
        dir_path,
        DEFAULT_BOOTSTRAP_NAME,
        BootstrapPolicy::Password(token.as_bytes()),
        &kdf,
    )
    .unwrap();

    let before = user_dir.lookup_by_name(DEFAULT_BOOTSTRAP_NAME).unwrap();
    assert!(
        DerivedKeys::derive(token.as_bytes(), &before.salt, &before.kdf_params)
            .map(|d| d.stored_key.0 == before.stored_key.0)
            .unwrap_or(false),
        "sanity: the token must verify BEFORE rotation"
    );

    let now_ns = UnixNanos::now().as_u64();
    rotate_bootstrap_credential_to_random(&user_dir, DEFAULT_BOOTSTRAP_NAME, kdf, now_ns)
        .await
        .expect("rotation must succeed");

    let after = user_dir.lookup_by_name(DEFAULT_BOOTSTRAP_NAME).unwrap();
    assert_ne!(
        before.salt, after.salt,
        "rotation must install a fresh random salt"
    );
    assert_ne!(
        before.stored_key.0, after.stored_key.0,
        "rotation must install a fresh random stored_key"
    );

    let would_still_verify = DerivedKeys::derive(token.as_bytes(), &after.salt, &after.kdf_params)
        .map(|d| d.stored_key.0 == after.stored_key.0)
        .unwrap_or(false);
    assert!(
        !would_still_verify,
        "the OLD token must no longer verify against the ROTATED record"
    );
}

/// F-3 regression: exercises the SAME "rotate-first → consume-on-success →
/// delete-file-last" ordering that `connection/handshake.rs` (first-login
/// path) and `server/server_launcher.rs` (TTL sweep) both apply. Proves
/// that a rotation failure leaves the token metadata row ACTIVE (not
/// consumed) and the token file on disk, so a retry — once the user
/// directory has the user — correctly consumes + deletes.
///
/// This test calls `rotate_bootstrap_credential_to_random` directly (it is
/// `pub(crate)`) against a real `ServerMetaStore` + `FjallUserDirectory`,
/// replicating the exact three-step sequence the production code performs.
/// The full end-to-end sweep-path version lives in
/// `tests/bootstrap_token_lifecycle_e2e.rs::
///  ttl_sweep_rotation_failure_leaves_token_active_for_retry`.
#[tokio::test]
async fn f3_rotation_failure_leaves_token_active_then_retry_consumes() {
    let dir = TempDir::new().unwrap();
    let dir_path = dir.path();
    let meta = ServerMetaStore::open_or_init(dir_path.join("server_meta")).unwrap();
    let user_dir = FjallUserDirectory::open(dir_path.join("users.redb")).unwrap();
    let kdf = fast_kdf();

    let token_path: PathBuf = dir_path.join("bootstrap_token.txt");
    fs::write(&token_path, b"the-bootstrap-token").unwrap();
    meta.set_bootstrap_token(
        DEFAULT_BOOTSTRAP_NAME,
        sha256(b"the-bootstrap-token"),
        u64::MAX, // far-future expiry — the ordering is the same regardless
        token_path.clone(),
    )
    .unwrap();
    assert!(meta.bootstrap_token_active());

    // --- Step 1: rotation FAILS (no "admin" user in the directory yet). ---
    // Per the F-3 reordering, the caller must NOT consume the token or
    // delete the file — leave them for a retry.
    let rotate_err = rotate_bootstrap_credential_to_random(
        &user_dir,
        DEFAULT_BOOTSTRAP_NAME,
        kdf,
        UnixNanos::now().as_u64(),
    )
    .await;
    assert!(
        rotate_err.is_err(),
        "rotation must fail when the user does not exist in the directory"
    );

    // The caller (handshake / sweep code) sees the Err and skips consume +
    // delete. Assert the token is still fully intact.
    assert!(
        meta.bootstrap_token_active(),
        "F-3: token must stay ACTIVE after rotation failure — not consumed"
    );
    assert!(
        token_path.exists(),
        "F-3: token file must NOT be deleted after rotation failure"
    );
    assert_eq!(
        meta.bootstrap_username().as_deref(),
        Some(DEFAULT_BOOTSTRAP_NAME),
        "F-3: bootstrap username must survive a rotation failure"
    );

    // --- Step 2: create the user, then retry — rotation now succeeds. ---
    ensure_superuser(
        &user_dir,
        dir_path,
        DEFAULT_BOOTSTRAP_NAME,
        BootstrapPolicy::Password(b"some-password"),
        &kdf,
    )
    .unwrap();

    let rotate_ok = rotate_bootstrap_credential_to_random(
        &user_dir,
        DEFAULT_BOOTSTRAP_NAME,
        kdf,
        UnixNanos::now().as_u64(),
    )
    .await;
    assert!(
        rotate_ok.is_ok(),
        "rotation must succeed once the user exists"
    );

    // Per the F-3 reordering: on rotate success, read the token path
    // BEFORE consume, then consume, then delete the file.
    let path_before_consume = meta.bootstrap_token_path();
    assert_eq!(
        path_before_consume.as_ref(),
        Some(&token_path),
        "token path must still be readable before consume"
    );
    meta.consume_bootstrap_token().unwrap();
    if let Some(p) = path_before_consume {
        let _ = fs::remove_file(&p);
    }

    assert!(
        !meta.bootstrap_token_active(),
        "F-3: token must be CONSUMED after successful rotation"
    );
    assert!(
        !token_path.exists(),
        "F-3: token file must be DELETED after successful rotation"
    );
}
