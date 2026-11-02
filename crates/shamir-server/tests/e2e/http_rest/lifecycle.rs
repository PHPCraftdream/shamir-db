use rustls::pki_types::pem::PemObject;
use shamir_connect::server::token::TokenMode;
use shamir_query_builder::{batch::Batch, ddl, Query};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use shamir_server::server::{BootstrapMode, ServerLauncher};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use super::common::{request, Harness, ADMIN_PASSWORD};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_tls_listener_serves_authenticated_ping() {
    let harness = super::common::Harness::launch_with(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(true),
            ..Default::default()
        },
    )
    .await;
    let token = harness.token("https-ping", TokenMode::ReadOnly).await;
    let pem = std::fs::read(harness.temp.path().join("cert.pem")).expect("read generated cert");
    let cert = rustls::pki_types::CertificateDer::pem_slice_iter(&pem)
        .next()
        .expect("certificate PEM entry")
        .expect("parse generated certificate PEM");
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).expect("trust generated certificate");
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut client_config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3 supported")
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = TlsConnector::from(std::sync::Arc::new(client_config));
    let tcp = TcpStream::connect(harness.addr())
        .await
        .expect("connect HTTP TLS");
    let mut tls = tokio::time::timeout(
        Duration::from_secs(15),
        connector.connect(
            rustls::pki_types::ServerName::try_from("localhost").unwrap(),
            tcp,
        ),
    )
    .await
    .expect("TLS handshake timed out")
    .expect("TLS handshake");
    assert_eq!(
        tls.get_ref().1.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]));

    let request = format!(
        "GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    tokio::time::timeout_at(deadline, tls.write_all(request.as_bytes()))
        .await
        .expect("HTTP request write timed out")
        .expect("write request");
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        assert!(response.len() < 8192, "HTTP response headers too large");
        let n = tokio::time::timeout_at(deadline, tls.read(&mut chunk))
            .await
            .expect("HTTP headers timed out")
            .expect("read HTTP headers");
        assert!(n > 0, "HTTP response ended before headers");
        response.extend_from_slice(&chunk[..n]);
        if let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
    };
    let headers = String::from_utf8_lossy(&response[..header_end]);
    assert!(
        headers.starts_with("HTTP/1.1 200"),
        "unexpected response: {headers}"
    );
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| {
                value
                    .trim()
                    .parse::<usize>()
                    .expect("numeric Content-Length")
            })
        })
        .expect("Content-Length response");
    assert!(
        content_length <= 1024 * 1024,
        "HTTP response body too large"
    );
    let body_start = header_end + 4;
    let body_end = body_start
        .checked_add(content_length)
        .expect("body size overflow");
    while response.len() < body_end {
        let n = tokio::time::timeout_at(deadline, tls.read(&mut chunk))
            .await
            .expect("HTTP body timed out")
            .expect("read HTTP body");
        assert!(n > 0, "HTTP response body ended early");
        response.extend_from_slice(&chunk[..n]);
        assert!(
            response.len() <= body_end,
            "unexpected trailing HTTP response data"
        );
    }
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&response[body_start..body_end]).expect("decode ping"),
        DbResponse::Pong
    ));
    drop(tls);
    let report = harness.handle.shutdown().await;
    assert!(report.is_clean(), "{report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_allowed_owner_read_and_shutdown_restart_preserve_data() {
    let harness = Harness::launch_with_native_seed().await;
    let client = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("admin TLS-exporter connect");

    let mut create = Batch::new();
    create.create_table("items", ddl::create_table("items"));
    client
        .execute("default", create.build())
        .await
        .expect("create table");
    let mut insert = Batch::new();
    insert.insert(
        "ins",
        shamir_query_builder::write::insert("items")
            .rows([shamir_query_builder::doc! { "marker" => "http-owner-read" }]),
    );
    client
        .execute("default", insert.build())
        .await
        .expect("insert seed row");
    drop(client);

    let token = harness.token("http-read", TokenMode::ReadOnly).await;
    let query = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.query("rd", Query::from("items"));
            batch.build()
        },
    };
    let request_body = rmp_serde::to_vec_named(&query).expect("encode read request");
    let (status, _, response_body) = request(harness.addr(), Some(&token), &request_body).await;
    assert_eq!(status, 200);
    let response =
        rmp_serde::from_slice::<DbResponse>(&response_body).expect("decode read response");
    let DbResponse::Batch { response } = response else {
        panic!("expected batch response: {response:?}");
    };
    assert_eq!(
        response.results.get("rd").expect("rd alias").records.len(),
        1
    );
    assert_eq!(
        response.results.get("rd").expect("rd alias").records[0].get_value_str("marker"),
        Some("http-owner-read")
    );

    let Harness {
        handle,
        temp,
        owner: _,
    } = harness;
    let report = handle.shutdown().await;
    assert!(report.is_clean(), "{report:?}");
    let config = super::common::config_for(temp.path());
    let restarted = ServerLauncher::new(config)
        .with_bootstrap(BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        })
        .launch()
        .await
        .expect("same-directory restart");
    let restarted_addr = restarted.bound_addrs[0].expect("restarted HTTP bound address");
    let (status, _, restarted_response_body) =
        request(restarted_addr, Some(&token), &request_body).await;
    assert_eq!(status, 200);
    let response = rmp_serde::from_slice::<DbResponse>(&restarted_response_body)
        .expect("decode restarted read");
    let DbResponse::Batch { response } = response else {
        panic!("expected batch response after restart: {response:?}");
    };
    assert_eq!(
        response.results.get("rd").expect("rd alias").records.len(),
        1
    );
    assert_eq!(
        response.results.get("rd").expect("rd alias").records[0].get_value_str("marker"),
        Some("http-owner-read")
    );
    assert!(restarted.shutdown().await.is_clean());
    drop(temp);
}
