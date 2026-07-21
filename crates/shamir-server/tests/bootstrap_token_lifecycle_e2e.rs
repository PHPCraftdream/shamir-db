//! RI-9 end-to-end regression: bootstrap-token lifecycle wiring.
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
//! Deliberately NOT tested here: whether the token still works as a
//! credential after consumption (it does — `changePassword` was never
//! called, so the SCRAM record itself is unchanged). Revoking the
//! credential itself is out of scope for this task; see the module-level
//! brief (`docs/dev-artifacts/prompts/correctness/03-bootstrap-token-lifecycle.md`).

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
