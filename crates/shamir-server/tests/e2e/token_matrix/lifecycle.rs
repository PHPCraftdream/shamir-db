use super::super::http_rest::common::{ping_body, request, Harness, ADMIN_PASSWORD};
use super::issuance::connect;
use super::support::DEADLINE;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::token::{TokenChannels, TokenId, TokenMode};
use shamir_query_builder::{batch::Batch, ddl, token_admin, Query};
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use shamir_server::backup;
use shamir_server::restore;
use shamir_server::server::{BootstrapMode, ServerLauncher};
use shamir_server::tokens::NewToken;
use shamir_server::user_directory::FjallUserDirectory;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use zeroize::Zeroizing;

async fn read_head(stream: &mut TcpStream) -> String {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut response_head = Vec::new();
    loop {
        assert!(response_head.len() < 8192, "response head too large");
        let mut byte = [0u8; 1];
        let n = tokio::time::timeout_at(deadline, stream.read(&mut byte))
            .await
            .expect("response head timeout")
            .expect("read response head");
        assert_ne!(n, 0, "connection closed before response head");
        response_head.push(byte[0]);
        if response_head.ends_with(b"\r\n\r\n") {
            return String::from_utf8_lossy(&response_head).into_owned();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preauth_declared_body_flood_does_not_consume_authenticated_body_capacity() {
    let harness = Harness::launch_with(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(false),
            max_request_body_bytes: 1024,
            body_timeout_ms: 30_000,
            request_timeout_ms: 120_000,
            ..Default::default()
        },
    )
    .await;
    let addr = harness.handle.bound_addrs[0].expect("HTTP address");
    let mut unauthenticated = Vec::new();
    for _ in 0..16 {
        let mut stream = tokio::time::timeout(DEADLINE, TcpStream::connect(addr))
            .await
            .expect("connect timeout")
            .expect("connect");
        let head = "POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 1024\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n";
        tokio::time::timeout(DEADLINE, stream.write_all(head.as_bytes()))
            .await
            .expect("head write timeout")
            .expect("write headers");
        // One body byte is enough to exercise a partial declared-body request.
        tokio::time::timeout(DEADLINE, stream.write_all(b"x"))
            .await
            .expect("body write timeout")
            .expect("write partial body");
        unauthenticated.push(stream);
    }
    for mut stream in unauthenticated {
        let response = read_head(&mut stream).await;
        assert!(
            response.starts_with("HTTP/1.1 401"),
            "unexpected response: {response}"
        );
        assert!(
            !response.contains("100 Continue"),
            "unauthenticated request received 100 Continue"
        );
        assert!(
            response.to_ascii_lowercase().contains("connection: close"),
            "connection was not closed: {response}"
        );
    }

    // The configured per-request cap is 1 KiB; the listener budget is 16 such
    // caps. Authenticated reservations fill it, so another authenticated head
    // is rejected. This is a behavioral capacity check, not a counter/peak-memory
    // measurement of the earlier unauthorized requests.
    let token = harness
        .token_with_options(
            "body-capacity",
            TokenMode::ReadOnly,
            3600,
            TokenChannels::Rest,
            None,
        )
        .await;
    let mut admitted = Vec::new();
    for _ in 0..16 {
        let mut stream = tokio::time::timeout(DEADLINE, TcpStream::connect(addr))
            .await
            .expect("connect timeout")
            .expect("connect");
        let head = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Type: application/msgpack\r\nContent-Length: 1024\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n", token.token.as_str());
        tokio::time::timeout(DEADLINE, stream.write_all(head.as_bytes()))
            .await
            .expect("head write timeout")
            .expect("write authenticated headers");
        let interim = read_head(&mut stream).await;
        assert!(
            interim.starts_with("HTTP/1.1 100"),
            "authenticated body was not admitted: {interim}"
        );
        admitted.push(stream);
    }
    let mut extra = tokio::time::timeout(DEADLINE, TcpStream::connect(addr))
        .await
        .expect("connect timeout")
        .expect("connect");
    let head = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Type: application/msgpack\r\nContent-Length: 1024\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n", token.token.as_str());
    tokio::time::timeout(DEADLINE, extra.write_all(head.as_bytes()))
        .await
        .expect("head write timeout")
        .expect("write extra headers");
    let rejected = read_head(&mut extra).await;
    assert!(
        rejected.starts_with("HTTP/1.1 503"),
        "expected authenticated capacity rejection: {rejected}"
    );
    drop(extra);
    let mut unauthenticated = tokio::time::timeout(DEADLINE, TcpStream::connect(addr))
        .await
        .expect("connect timeout")
        .expect("connect");
    let head = "POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 1024\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n";
    tokio::time::timeout(DEADLINE, unauthenticated.write_all(head.as_bytes()))
        .await
        .expect("head write timeout")
        .expect("write unauthorized headers");
    let unauthorized = read_head(&mut unauthenticated).await;
    assert!(
        unauthorized.starts_with("HTTP/1.1 401"),
        "expected unauthorized response after capacity exhaustion: {unauthorized}"
    );
    drop(unauthenticated);
    drop(admitted);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let (status, _, body) = tokio::time::timeout_at(
            deadline,
            request(addr, Some(token.token.as_str()), &ping_body()),
        )
        .await
        .expect("reservation release timeout");
        if status == 200 {
            assert!(matches!(
                rmp_serde::from_slice::<DbResponse>(&body).expect("successful ping response"),
                DbResponse::Pong
            ));
            break;
        }
        assert_eq!(
            status, 503,
            "unexpected response while waiting for reservation release"
        );
        tokio::task::yield_now().await;
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(45), harness.handle.shutdown())
            .await
            .expect("shutdown timeout")
            .errors()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_revoke_invalidates_online_token() {
    let harness = Harness::launch_with_native_seed().await;
    let admin = connect(&harness, "admin", ADMIN_PASSWORD).await;
    let created = admin
        .create_api_token(
            token_admin::create_api_token("lifecycle")
                .ttl_secs(3600)
                .mode(ApiTokenMode::ReadWrite)
                .channels(ApiTokenChannels::Rest),
        )
        .await
        .expect("create token");
    let token = created.token.reveal().to_string();
    let mut create = Batch::new();
    create.create_table("items", ddl::create_table("items"));
    admin
        .execute("default", create.build())
        .await
        .expect("create table");
    let mut seed = Batch::new();
    seed.insert(
        "seed",
        shamir_query_builder::write::insert("items")
            .rows([shamir_query_builder::doc! {"marker" => "positive-seed"}]),
    );
    admin
        .execute("default", seed.build())
        .await
        .expect("insert seed");

    let mut insert = Batch::new();
    insert.insert(
        "attempt",
        shamir_query_builder::write::insert("items")
            .rows([shamir_query_builder::doc! {"marker" => "must-not-commit"}]),
    );
    let body = rmp_serde::to_vec_named(&DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: insert.build(),
    })
    .expect("encode insert request");
    let mut stream = tokio::time::timeout(DEADLINE, TcpStream::connect(harness.addr()))
        .await
        .expect("connect timeout")
        .expect("connect");
    let head = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/msgpack\r\nExpect: 100-continue\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    tokio::time::timeout(DEADLINE, stream.write_all(head.as_bytes()))
        .await
        .expect("head write timeout")
        .expect("write head");
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut response_head = Vec::new();
    loop {
        assert!(response_head.len() < 8192, "response head too large");
        let mut byte = [0u8; 1];
        let n = tokio::time::timeout_at(deadline, stream.read(&mut byte))
            .await
            .expect("100 response timeout")
            .expect("read response head");
        assert_ne!(n, 0, "connection closed before 100 response");
        response_head.push(byte[0]);
        if response_head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    assert!(
        String::from_utf8_lossy(&response_head).starts_with("HTTP/1.1 100"),
        "unexpected interim response: {}",
        String::from_utf8_lossy(&response_head)
    );
    let half = body.len() / 2;
    tokio::time::timeout(DEADLINE, stream.write_all(&body[..half]))
        .await
        .expect("first half write timeout")
        .expect("write first half");
    admin
        .revoke_api_token(&created.id)
        .await
        .expect("revoke token");
    tokio::time::timeout(DEADLINE, stream.write_all(&body[half..]))
        .await
        .expect("finish body timeout")
        .expect("finish body");
    let mut full_head = response_head;
    loop {
        assert!(full_head.len() < 8192, "final response head too large");
        let mut byte = [0u8; 1];
        let n = tokio::time::timeout_at(deadline, stream.read(&mut byte))
            .await
            .expect("final response timeout")
            .expect("read final response");
        assert_ne!(n, 0, "closed before final response");
        full_head.push(byte[0]);
        if full_head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head_start = full_head
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("interim head")
        + 4;
    let final_head = String::from_utf8_lossy(&full_head[head_start..]).to_string();
    assert!(
        final_head.starts_with("HTTP/1.1 401"),
        "unexpected final response: {}",
        final_head
    );
    let body_len =
        super::support::content_length(&final_head).expect("final response content-length");
    assert!(
        body_len <= 1024,
        "final response body too large: {body_len}"
    );
    let mut final_body = vec![0; body_len];
    tokio::time::timeout(DEADLINE, stream.read_exact(&mut final_body))
        .await
        .expect("final response body timeout")
        .expect("read final response body");
    assert_eq!(final_body, b"\x81\xa5error\xacunauthorized");
    drop(stream);
    let query = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.query("rows", Query::from("items"));
            batch.build()
        },
    };
    let (status, _, bytes) = request(
        harness.addr(),
        Some(
            &admin
                .create_api_token(
                    token_admin::create_api_token("read-check")
                        .mode(ApiTokenMode::ReadOnly)
                        .channels(ApiTokenChannels::Rest),
                )
                .await
                .expect("read control token")
                .token
                .reveal()
                .to_owned(),
        ),
        &rmp_serde::to_vec_named(&query).expect("encode query"),
    )
    .await;
    assert_eq!(status, 200);
    let DbResponse::Batch { response } = rmp_serde::from_slice(&bytes).expect("decode read") else {
        panic!("expected batch response")
    };
    let records = &response.results.get("rows").expect("rows result").records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get_value_str("marker"), Some("positive-seed"));
    drop(admin);
    assert!(
        tokio::time::timeout(Duration::from_secs(45), harness.handle.shutdown())
            .await
            .expect("shutdown timeout")
            .errors()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_invalidates_tokens_and_allows_fresh_issuance() {
    let restore_root = tempfile::TempDir::new().expect("restore data parent");
    let data_dir = restore_root.path().join("data");
    std::fs::create_dir(&data_dir).expect("create data directory");
    let (owner, owner_roles) = super::super::http_rest::common::seed_owner(&data_dir).await;
    let handle = ServerLauncher::new(super::super::http_rest::common::config_for(&data_dir))
        .with_bootstrap(BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        })
        .launch()
        .await
        .expect("launch HTTP listener");
    let token = handle
        .token_service
        .create(NewToken {
            owner_user_id: owner,
            owner_roles: owner_roles.clone(),
            label: "restore".into(),
            ttl_secs: Some(3600),
            mode: TokenMode::ReadOnly,
            channels: TokenChannels::Rest,
            databases: None,
        })
        .await
        .expect("create token");
    let (status, _, _) = request(
        handle.bound_addrs[0].expect("HTTP address"),
        Some(token.token.as_str()),
        &ping_body(),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        tokio::time::timeout(Duration::from_secs(45), handle.shutdown())
            .await
            .expect("shutdown timeout")
            .errors()
            .is_empty()
    );
    let snapshot_dir = tempfile::TempDir::new().expect("snapshot parent");
    let snapshot =
        backup::backup(&data_dir, snapshot_dir.path()).expect("backup stopped data directory");
    let server = ServerLauncher::new(super::super::http_rest::common::config_for(&data_dir))
        .with_bootstrap(BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        })
        .launch()
        .await
        .expect("restart before restore");
    let (status, _, _) = request(
        server.bound_addrs[0].expect("HTTP address"),
        Some(token.token.as_str()),
        &ping_body(),
    )
    .await;
    assert_eq!(status, 200);
    let parsed = server
        .token_service
        .lookup(token.record_view.id)
        .await
        .expect("lookup token")
        .expect("stored token");
    assert!(
        parsed.revoked_at_ns.is_none(),
        "token should be active before revoke"
    );
    let revoked_at = UnixNanos::try_now().expect("clock").as_u64();
    server
        .token_service
        .revoke(TokenId(parsed.id), revoked_at)
        .await
        .expect("online revoke");
    let (status, _, _) = request(
        server.bound_addrs[0].expect("HTTP address"),
        Some(token.token.as_str()),
        &ping_body(),
    )
    .await;
    assert_eq!(status, 401);
    assert!(server.shutdown().await.errors().is_empty());
    let report = restore::restore(&snapshot.dest_dir, &data_dir, false).expect("restore snapshot");
    assert!(
        report.users_invalidated > 0,
        "expected invalidated users: {report:?}"
    );
    let directory =
        FjallUserDirectory::open(data_dir.join("users")).expect("reopen user directory");
    let owner_roles = directory
        .state_by_user_id(&owner)
        .expect("read owner state")
        .roles;
    assert!(
        directory.tickets_invalid_before_ns_by_user_id(&owner) > token.record_view.created_at_ns
    );
    drop(directory);
    let restarted = ServerLauncher::new(super::super::http_rest::common::config_for(&data_dir))
        .with_bootstrap(BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        })
        .launch()
        .await
        .expect("restart restored data");
    let addr = restarted.bound_addrs[0].expect("HTTP address");
    let (status, _, _) = request(addr, Some(token.token.as_str()), &ping_body()).await;
    assert_eq!(status, 401);
    let fresh = restarted
        .token_service
        .create(NewToken {
            owner_user_id: owner,
            owner_roles,
            label: "fresh".into(),
            ttl_secs: Some(3600),
            mode: TokenMode::ReadOnly,
            channels: TokenChannels::Rest,
            databases: None,
        })
        .await
        .expect("fresh token");
    let (status, _, _) = request(addr, Some(fresh.token.as_str()), &ping_body()).await;
    assert_eq!(status, 200);
    assert!(restarted.shutdown().await.errors().is_empty());
}
