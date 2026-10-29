use crate::common;

use std::time::Duration;

use shamir_client::{Client, ClientError, ConnectOptions};
use shamir_connect::common::crypto::{sha256, Ed25519Keypair};
use shamir_server::config::KdfConfig;
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};
use shamir_server::server_meta::ServerMetaStore;
use tempfile::TempDir;
use zeroize::Zeroizing;

const PASSWORD: &[u8] = b"auth-ok-hints-e2e-password";

fn client_options(addr: std::net::SocketAddr, pin: Option<[u8; 32]>) -> ConnectOptions {
    ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(PASSWORD.to_vec()),
        accept_new_host: pin.is_none(),
        trusted_pin: pin,
        connect_timeout: Some(Duration::from_secs(5)),
        handshake_timeout: Some(Duration::from_secs(30)),
        request_timeout: Some(Duration::from_secs(10)),
    }
}

async fn launch(temp: &TempDir, kdf_defaults: KdfConfig) -> ServerHandle {
    let mut config = common::make_test_config(temp, "127.0.0.1:0");
    config.kdf_defaults = kdf_defaults;
    // The boot warmup quarters the pre-auth bucket (2 tokens) and shutdown
    // persists it, so a relaunch plus several reconnects would be rate limited.
    config.security.auth_init_rate_per_second = 1_000;
    ServerLauncher {
        config,
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(PASSWORD.to_vec()),
        },
    }
    .launch()
    .await
    .expect("server launch")
}

fn strong_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 20_480,
        time: 3,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_f20c09bf_kdf_upgrade_hint_e2e() {
    let temp = TempDir::new().expect("tempdir");
    let weak = common::fast_kdf();
    let handle = launch(&temp, weak).await;
    let first = Client::connect(client_options(
        handle.first_tls_exporter_addr().expect("bound"),
        None,
    ))
    .await
    .expect("create weak-KDF user");
    assert!(!first.kdf_upgrade_required());
    first.close().await;
    handle.shutdown().await;

    let handle = launch(&temp, strong_kdf()).await;
    let upgraded = Client::connect(client_options(
        handle.first_tls_exporter_addr().expect("bound"),
        None,
    ))
    .await
    .expect("login with old KDF credential");
    assert!(upgraded.kdf_upgrade_required());
    upgraded.close().await;
    handle.shutdown().await;

    let fresh_temp = TempDir::new().expect("fresh data dir");
    let fresh_handle = launch(&fresh_temp, strong_kdf()).await;
    let fresh = Client::connect(client_options(
        fresh_handle.first_tls_exporter_addr().expect("bound"),
        None,
    ))
    .await
    .expect("fresh strong-KDF user");
    assert!(!fresh.kdf_upgrade_required());
    fresh.close().await;
    fresh_handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_f20c09bf_rotation_pending_e2e() {
    let temp = TempDir::new().expect("tempdir");
    let handle = launch(&temp, common::fast_kdf()).await;
    let addr = handle.first_tls_exporter_addr().expect("bound");
    let first = Client::connect(client_options(addr, None))
        .await
        .expect("initial login");
    let old_pin = first.server_pub_key_pin();
    first.close().await;
    handle.shutdown().await;

    let store = ServerMetaStore::open_or_init(temp.path().join("server_meta")).expect("meta");
    let old_seed = store.current_identity_seed().expect("old seed");
    let new_seed = [0x6bu8; 32];
    let transition_until_ns = shamir_connect::common::time::UnixNanos::now()
        .saturating_add(7 * shamir_connect::common::time::ns::DAY)
        .as_u64();
    store
        .store_identity_after_rotate(new_seed, old_seed, transition_until_ns, 1)
        .expect("rotate identity");
    drop(store);
    let new_pin = sha256(&Ed25519Keypair::from_seed(&new_seed).public_bytes());

    let handle = launch(&temp, common::fast_kdf()).await;
    let addr = handle.first_tls_exporter_addr().expect("bound");
    match Client::connect(client_options(addr, Some(old_pin))).await {
        Err(ClientError::IdentityRotationPending {
            previous_pin,
            new_pin: observed_new,
            transition_until_ns: observed_until,
        }) => {
            assert_eq!(previous_pin, old_pin);
            assert_eq!(observed_new, new_pin);
            assert_eq!(observed_until, transition_until_ns);
        }
        other => panic!("expected pending rotation, got {:?}", other.err()),
    }
    let approved = Client::connect(client_options(addr, Some(new_pin)))
        .await
        .expect("approved new pin");
    assert_eq!(approved.server_pub_key_pin(), new_pin);
    approved.close().await;
    let unrelated = Client::connect(client_options(addr, Some([0xa5; 32]))).await;
    assert!(matches!(
        unrelated,
        Err(ClientError::Handshake { ref context, .. })
            if context.contains("server_identity_changed")
    ));
    handle.shutdown().await;
}
