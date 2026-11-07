use super::common::{begin_body, finish_body, LoginChallenge, PASSWORD, USERNAME};
use rustls::pki_types::pem::PemObject;
use shamir_server::config::HttpListenerSettings;
use shamir_server::server::{BootstrapMode, ServerLauncher};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use zeroize::Zeroizing;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_login_tls_hsts_and_restart_preserve_listener_identity_but_drop_pending_state() {
    let mut settings = HttpListenerSettings {
        tls: Some(true),
        ..Default::default()
    };
    settings.login.enabled = true;
    let harness = super::common::Harness::launch_with_hooks(
        "127.0.0.1:0",
        settings,
        true,
        |_| {},
        |path| {
            super::common::seed_account(path, USERNAME, super::common::PASSWORD);
        },
    )
    .await;
    let addr = harness.addr();
    let pem =
        std::fs::read(harness.temp.path().join("cert.pem")).expect("read generated certificate");
    let cert = rustls::pki_types::CertificateDer::pem_slice_iter(&pem)
        .next()
        .expect("certificate PEM entry")
        .expect("parse certificate");
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).expect("trust generated test certificate");
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut client_config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(client_config));
    let (first_id, pending_id, pending_proof) = {
        let (status, headers, body) = tls_post(
            &connector,
            addr,
            "/shamir/v1/auth/begin",
            &begin_body("tls-pending", &[9; 32]),
        )
        .await;
        assert_eq!(status, 200);
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("strict-transport-security: max-age=31536000"),
            "TLS login response must carry exact HSTS: {headers}"
        );
        let challenge: LoginChallenge = rmp_serde::from_slice(&body).expect("decode TLS challenge");
        assert_eq!(challenge.security, 1);
        let id: [u8; 16] = challenge
            .listener_id
            .as_ref()
            .try_into()
            .expect("listener ID");
        let pending: [u8; 32] = challenge
            .handshake_id
            .as_ref()
            .try_into()
            .expect("handshake ID");
        let proof =
            super::common::proof_for(USERNAME, "tls-pending", "read_only", PASSWORD, &challenge);
        (id, pending, proof)
    };
    let super::common::Harness { handle, temp, .. } = harness;
    let report = handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
    let mut config = super::super::http_rest::common::config_for(temp.path());
    config.listeners[0].http.tls = Some(true);
    config.listeners[0].http.login.enabled = true;
    let restarted = ServerLauncher {
        config,
        node_mode: Default::default(),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(super::super::http_rest::common::ADMIN_PASSWORD.to_vec()),
        },
    }
    .launch()
    .await
    .expect("restart same data directory");
    let restarted_addr = restarted.bound_addrs[0].expect("restarted HTTP listener");
    let (status, headers, challenge) = tls_post(
        &connector,
        restarted_addr,
        "/shamir/v1/auth/begin",
        &begin_body("tls-restarted", &[9; 32]),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("strict-transport-security: max-age=31536000"),
        "restarted TLS response retains exact HSTS"
    );
    let challenge: LoginChallenge =
        rmp_serde::from_slice(&challenge).expect("decode restarted challenge");
    assert_eq!(challenge.security, 1);
    assert_eq!(
        challenge.listener_id.as_ref(),
        first_id,
        "logical listener identity is stable across restart"
    );
    let (status, _, body) = tls_post(
        &connector,
        restarted_addr,
        "/shamir/v1/auth/finish",
        &finish_body(&pending_id, &pending_proof),
    )
    .await;
    assert_eq!(
        status, 401,
        "pre-restart valid pending proof must be rejected after restart"
    );
    let denied: ErrorBody = rmp_serde::from_slice(&body).expect("decode pending-state denial");
    assert_eq!(denied.error, "authentication_failed");
    let new_id: [u8; 32] = challenge
        .handshake_id
        .as_ref()
        .try_into()
        .expect("new handshake ID");
    let new_proof =
        super::common::proof_for(USERNAME, "tls-restarted", "read_only", PASSWORD, &challenge);
    let (status, _, _) = tls_post(
        &connector,
        restarted_addr,
        "/shamir/v1/auth/finish",
        &finish_body(&new_id, &new_proof),
    )
    .await;
    assert_eq!(status, 200, "fresh TLS login succeeds after restart");
    let report = restarted.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
    drop(temp);
}

async fn tls_post(
    connector: &tokio_rustls::TlsConnector,
    addr: SocketAddr,
    path: &str,
    body: &[u8],
) -> (u16, String, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(15), async {
        assert!(path.starts_with('/') && !path.contains(['\r', '\n', '\0']));
        let tcp = TcpStream::connect(addr)
            .await
            .expect("connect TLS listener");
        let server_name =
            rustls::pki_types::ServerName::try_from("localhost").expect("server name");
        let mut stream = connector
            .connect(server_name, tcp)
            .await
            .expect("validated TLS handshake");
        tls_post_inner(&mut stream, addr, path, body).await
    })
    .await
    .expect("TLS request timed out")
}

async fn tls_post_inner(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    addr: SocketAddr,
    path: &str,
    body: &[u8],
) -> (u16, String, Vec<u8>) {
    let request = format!("POST {path} HTTP/1.1\r\nHost: localhost:{}\r\nContent-Type: application/msgpack\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", addr.port(), body.len());
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write TLS headers");
    stream.write_all(body).await.expect("write TLS body");
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    let split = loop {
        assert!(response.len() < 8192, "TLS response headers exceed bound");
        let n = stream.read(&mut chunk).await.expect("read TLS response");
        assert!(n > 0, "TLS response ended before headers");
        response.extend_from_slice(&chunk[..n]);
        if let Some(split) = response.windows(4).position(|window| window == b"\r\n\r\n") {
            break split;
        }
    };
    let headers = String::from_utf8_lossy(&response[..split]).into_owned();
    let status = headers
        .split_whitespace()
        .nth(1)
        .expect("HTTP status")
        .parse()
        .expect("numeric status");
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .expect("Content-Length");
    assert!(length <= 4096, "response body within protocol bound");
    let start = split + 4;
    let end = start.checked_add(length).expect("response length");
    while response.len() < end {
        let n = stream
            .read(&mut chunk)
            .await
            .expect("read TLS response body");
        assert!(n > 0, "TLS response body truncated");
        response.extend_from_slice(&chunk[..n]);
        assert!(response.len() <= end, "unexpected trailing response bytes");
    }
    (status, headers, response[start..end].to_vec())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorBody {
    error: String,
}
