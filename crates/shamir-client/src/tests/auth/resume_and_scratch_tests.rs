use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};
use tokio::io::duplex;

use crate::{Client, ConnectOptions, ResumeOptions, CURRENT_QUERY_LANG_VERSION};

fn config(temp: &TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".into(),
            profile: ProfileKind::TlsExporter,
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

async fn start() -> (
    TempDir,
    shamir_server::server::ServerHandle,
    std::net::SocketAddr,
) {
    let temp = TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();
    let handle = ServerLauncher {
        config: config(&temp),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password),
        },
    }
    .launch()
    .await
    .expect("server launch");
    let addr = handle.first_tls_exporter_addr().expect("bound listener");
    (temp, handle, addr)
}

fn connect_options(addr: std::net::SocketAddr, pin: Option<[u8; 32]>) -> ConnectOptions {
    ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(b"correct horse battery staple".to_vec()),
        accept_new_host: pin.is_none(),
        trusted_pin: pin,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_2d08f353_public_resume_after_rejected_pin() {
    let (_temp, handle, addr) = start().await;
    let initial = Client::connect(connect_options(addr, None))
        .await
        .expect("initial connect");
    let ticket = initial
        .resumption_ticket()
        .expect("resumption ticket")
        .to_vec();
    let pin = initial.server_pub_key_pin();
    let session_id = initial.session_id();
    initial.close().await;

    let rejected = Client::resume(ResumeOptions {
        addr,
        server_name: "localhost".into(),
        ticket: ticket.clone(),
        pinned_hash: [0xA5; 32],
    })
    .await;
    assert!(matches!(
        rejected,
        Err(crate::ClientError::Handshake { ref context, .. })
            if context.contains("identity does not match pinned hash")
    ));

    // Three connects right after boot exceed the warm-up per-subnet accept
    // budget; a rate-limited connection is dropped before the proof frame, so
    // the ticket is never disclosed and a retry is safe.
    let mut attempts = 0;
    let resumed = loop {
        attempts += 1;
        match Client::resume(ResumeOptions {
            addr,
            server_name: "localhost".into(),
            ticket: ticket.clone(),
            pinned_hash: pin,
        })
        .await
        {
            Err(crate::ClientError::Transport { context, .. })
                if context == "read resume_proof_ok" && attempts < 20 =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            other => break other.expect("resume with trusted pin"),
        }
    };
    assert_ne!(resumed.session_id(), session_id);
    assert_eq!(resumed.server_pub_key_pin(), pin);
    assert_eq!(
        u32::from(resumed.server_query_version()),
        CURRENT_QUERY_LANG_VERSION
    );
    resumed.ping().await.expect("ping resumed client");
    resumed.close().await;
    handle.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn rv_client_150f9230_request_scratch_empty_after_secret_send() {
    // A current-thread runtime guarantees this request and its server peer use
    // the same thread-local scratch buffer.
    let (client_write, server_read) = duplex(64 * 1024);
    let (client_read, server_write) = duplex(64);
    let client = Client::for_test(
        client_read,
        client_write,
        crate::client::LifecycleHooks::default(),
    );
    let send = client.create_scram_user("u", Zeroizing::new("pw-150f9230-secret".into()), vec![]);
    let mut receive_read = server_read;
    let receive = async move {
        let frame = shamir_transport_tcp::framing::read_frame(
            &mut receive_read,
            shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT,
        )
        .await
        .expect("request frame");
        assert!(frame
            .windows(b"pw-150f9230-secret".len())
            .any(|w| w == b"pw-150f9230-secret"));
        drop(receive_read);
        drop(server_write);
    };
    let (result, ()) = tokio::join!(send, receive);
    assert!(matches!(
        result,
        Err(crate::ClientError::ConnectionClosed(_))
    ));
    assert_eq!(client.request_scratch_len_for_test(), 0);
    client.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn rv_client_150f9230_request_scratch_capacity_wiped_after_secret_send() {
    // A current-thread runtime guarantees this request and its server peer use
    // the same thread-local scratch buffer.
    let (client_write, server_read) = duplex(64 * 1024);
    let (client_read, server_write) = duplex(64);
    let client = Client::for_test(
        client_read,
        client_write,
        crate::client::LifecycleHooks::default(),
    );
    let capacity = client.fill_request_scratch_capacity_for_test();
    let send = client.create_scram_user("u", Zeroizing::new("pw-150f9230-secret".into()), vec![]);
    let mut receive_read = server_read;
    let receive = async move {
        let frame = shamir_transport_tcp::framing::read_frame(
            &mut receive_read,
            shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT,
        )
        .await
        .expect("request frame");
        assert!(frame
            .windows(b"pw-150f9230-secret".len())
            .any(|w| w == b"pw-150f9230-secret"));
        drop(receive_read);
        drop(server_write);
    };
    let (result, ()) = tokio::join!(send, receive);
    assert!(matches!(
        result,
        Err(crate::ClientError::ConnectionClosed(_))
    ));
    // SAFETY: on this current-thread runtime the fill helper initialized every
    // capacity byte. Roundtrip may grow the Vec, but the reader checks capacity
    // against its original value before reading; Vec writes grow only past capacity.
    let scratch = unsafe { client.request_scratch_capacity_bytes_for_test(capacity) };
    assert!(scratch.iter().all(|&byte| byte == 0));
    assert!(!scratch
        .windows(b"pw-150f9230-secret".len())
        .any(|window| window == b"pw-150f9230-secret"));
    client.close().await;
}
