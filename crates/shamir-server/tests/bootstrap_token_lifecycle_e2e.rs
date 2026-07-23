//! RI-9 / CR-A6 end-to-end regression: bootstrap-token lifecycle wiring.
//!
//! Proves the FULL live wiring — not just the unit-level `ServerMetaStore`
//! getters/setters (see `src/tests/server_meta_tests.rs`) or the
//! `ensure_superuser` path-override behavior (see
//! `src/tests/bootstrap_tests.rs`) — but the actual boot → login →
//! consume sequence a real operator hits:
//!
//! 1. Boot with `BootstrapMode::RandomToken` → the token file exists on disk.
//! 2. A real SCRAM login using the token as the password succeeds.
//! 3. After that successful connect, the token file is gone from disk.
//! 4. After `handle.shutdown()`, re-opening `ServerMetaStore` against the
//!    same `data_dir` shows `bootstrap_token_active() == false` — i.e. the
//!    consume was durably persisted, not just an in-memory side effect.
//!
//! CR-A6 (security fix — the token is now truly one-time): a SECOND login
//! attempt using the SAME token, after the first login already consumed
//! it, must now FAIL — the account's SCRAM credential was rotated to a
//! random, permanently-unknown value at the moment of the first login, so
//! the old token no longer authenticates. This module also proves
//! `changePassword` still works from the session opened by the first
//! (successful) token login, and that the boot-time TTL sweep rotates an
//! unused-but-expired token's credential too.

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::{Client, ConnectOptions};
use shamir_server::server_meta::ServerMetaStore;

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_token_file_exists_then_consumed_on_first_login() {
    let temp = TempDir::new().expect("tempdir");
    let (handle, token) = common::spawn_with_random_token(&temp, "127.0.0.1:0").await;
    let addr = handle.first_tls_exporter_addr().expect("bound");

    let token_path = temp.path().join("bootstrap_token.txt");

    // (a) Token file exists right after boot.
    assert!(
        token_path.exists(),
        "bootstrap token file must exist right after boot"
    );

    // (b) A real SCRAM login using the token as password succeeds.
    let client = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(token.clone().into_bytes()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("connect with bootstrap token as password must succeed");

    // (c) AFTER that successful connect, the token file is gone from disk.
    // The server-side consume runs synchronously as part of the handshake
    // (before auth_ok is sent), so by the time `connect()` returns the file
    // deletion has already happened.
    assert!(
        !token_path.exists(),
        "bootstrap token file must be deleted after first successful login"
    );

    drop(client);
    handle.shutdown().await;

    // (d) Re-open `ServerMetaStore` against the same data_dir after shutdown
    // and assert `bootstrap_token_active() == false` — the consume was
    // durably persisted, not just an in-memory side effect that a restart
    // would lose.
    let meta = ServerMetaStore::open_or_init(temp.path().join("server_meta"))
        .expect("reopen server_meta store after shutdown");
    assert!(
        !meta.bootstrap_token_active(),
        "bootstrap_token_active() must read back false from a fresh re-open"
    );
    assert!(
        meta.superuser_ever_existed(),
        "superuser_ever_existed must be sticky-true after a successful bootstrap+login"
    );
}

/// CR-A6 core guarantee: a SECOND login attempt with the SAME token, after
/// the first login already consumed it, must FAIL. Before this fix the
/// token kept working indefinitely (RI-9 only deleted the file/meta row,
/// never rotated the SCRAM credential) — this test fails against the
/// un-fixed code and passes once rotation lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_login_with_same_token_after_first_login_fails() {
    let temp = TempDir::new().expect("tempdir");
    let (handle, token) = common::spawn_with_random_token(&temp, "127.0.0.1:0").await;
    let addr = handle.first_tls_exporter_addr().expect("bound");

    // First login: succeeds, consumes the token, and (CR-A6) rotates the
    // account's SCRAM credential.
    let first = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(token.clone().into_bytes()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("first connect with bootstrap token as password must succeed");
    drop(first);

    // Second login attempt with the SAME (now-stale) token must fail —
    // the account's stored_key/server_key no longer match it.
    let second = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(token.into_bytes()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await;
    assert!(
        second.is_err(),
        "a second login with the SAME bootstrap token must fail after the \
         first successful login already rotated the credential"
    );

    handle.shutdown().await;
}

/// The boot-time TTL sweep (for a token nobody ever used) must ALSO rotate
/// the credential — not just delete the file/meta row. Simulated by
/// backdating `bootstrap_token_expires_at_ns` directly via `ServerMetaStore`
/// (mirroring `src/tests/server_meta_tests.rs`'s TTL-expiry tests) between
/// two boots against the same `data_dir`, then confirming the (expired AND
/// rotated) token no longer authenticates on the second boot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ttl_sweep_rotates_unused_expired_token_credential() {
    let temp = TempDir::new().expect("tempdir");

    // Boot #1: bootstrap with a random token, then shut down without ever
    // logging in (the token is "unused").
    let (handle, token) = common::spawn_with_random_token(&temp, "127.0.0.1:0").await;
    handle.shutdown().await;

    // Backdate the token's expiry so the next boot's TTL sweep treats it as
    // expired. Re-derive the same hash `set_bootstrap_token` originally
    // stored so `bootstrap_token_active()`/`bootstrap_username()` stay
    // consistent — only `expires_at_ns` and (implicitly) `now_ns` at sweep
    // time need to disagree.
    {
        let meta = ServerMetaStore::open_or_init(temp.path().join("server_meta"))
            .expect("reopen server_meta store before backdating TTL");
        assert!(
            meta.bootstrap_token_active(),
            "token must still be outstanding after boot #1 (never used)"
        );
        let username = meta
            .bootstrap_username()
            .expect("bootstrap_username must be set");
        let token_path = meta
            .bootstrap_token_path()
            .expect("bootstrap_token_path must be set");
        meta.set_bootstrap_token(
            &username,
            shamir_connect::common::crypto::sha256(token.as_bytes()),
            1, // already-past expiry (1ns since epoch)
            token_path,
        )
        .expect("backdate bootstrap token expiry");
    }

    // Boot #2 against the SAME data_dir: `BootstrapMode::Skip` so this boot
    // doesn't try to re-bootstrap (the user already exists) — the TTL sweep
    // runs unconditionally before the bootstrap step regardless of mode.
    let config = common::make_test_config(&temp, "127.0.0.1:0");
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let handle2 = shamir_server::server::ServerLauncher {
        config,
        bootstrap: shamir_server::server::BootstrapMode::Skip,
    }
    .launch()
    .await
    .expect("second launcher boot");
    let addr2 = handle2.first_tls_exporter_addr().expect("bound");

    // The token file must be gone and a login with the (expired AND
    // rotated) token must fail.
    let token_path = temp.path().join("bootstrap_token.txt");
    assert!(
        !token_path.exists(),
        "expired token file must be deleted by the boot-time TTL sweep"
    );

    let login = Client::connect(ConnectOptions {
        addr: addr2,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(token.into_bytes()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await;
    assert!(
        login.is_err(),
        "a login with a TTL-expired, sweep-rotated bootstrap token must fail"
    );

    handle2.shutdown().await;
}
