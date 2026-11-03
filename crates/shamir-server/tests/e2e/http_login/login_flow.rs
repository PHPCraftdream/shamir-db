use super::common::{finish_body, login, post, token_scope, PASSWORD, USERNAME};
use shamir_connect::server::token::{parse_token, TokenChannels, TokenMode};
use shamir_query_builder::{batch::Batch, ddl, write, Query};
use shamir_query_types::admin::ChmodOp;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::hmac::{canonical_batch_op, compute_tag_hex, derive_session_hmac_key};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use zeroize::Zeroizing;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_login_success_and_rejections() {
    let harness = super::common::launch().await;
    let client = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(crate::http_rest::common::ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("admin TLS-exporter connect");
    let mut create = Batch::new();
    create.create_table("login_items", ddl::create_table("login_items"));
    client
        .execute("default", create.build())
        .await
        .expect("create table");
    crate::http_rest::common::grant_default_main_access(&client).await;
    let resource = ddl::res::table("default", "main", "login_items");
    let unsigned = BatchOp::Chmod(ChmodOp {
        chmod: resource.clone(),
        mode: 0o744,
        hmac: None,
    });
    let tag = compute_tag_hex(
        &derive_session_hmac_key(&client.session_id()),
        &canonical_batch_op("default", &unsigned).expect("chmod canonical form"),
    );
    let mut chmod = Batch::new();
    chmod.chmod(
        "chmod_table",
        ddl::chmod(resource, 0o744)
            .hmac(tag)
            .build()
            .expect("build chmod"),
    );
    client
        .execute("default", chmod.build())
        .await
        .expect("grant table read");
    let mut insert = Batch::new();
    insert.insert(
        "ins",
        write::insert("login_items")
            .rows([shamir_query_builder::doc! { "marker" => "http-login-bearer-read" }]),
    );
    client
        .execute("default", insert.build())
        .await
        .expect("insert seed row");
    drop(client);
    let addr = harness.addr();
    let success = login(addr, "e2e-login", PASSWORD).await.expect("login");
    assert_eq!(success.protocol_version, 1);
    assert_eq!(success.label, "e2e-login");
    assert_eq!(success.owner, USERNAME);
    assert!(success.expires_at_ns > success.created_at_ns);
    let parsed = parse_token(success.token.as_bytes()).expect("parse token");
    assert_eq!(parsed.id.0.as_slice(), success.id.as_ref());
    assert!(success.expires_at_ns - success.created_at_ns <= 86_400_000_000_000);
    let ping_body = rmp_serde::to_vec_named(&DbRequest::Ping).expect("encode ping");
    let (status, _, body) =
        super::super::http_rest::common::request(addr, Some(&success.token), &ping_body).await;
    assert_eq!(status, 200);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&body).expect("decode ping response"),
        DbResponse::Pong
    ));
    let query = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.query("rd", Query::from("login_items"));
            batch.build()
        },
    };
    let request_body = rmp_serde::to_vec_named(&query).expect("encode read request");
    let (status, _, body) =
        super::super::http_rest::common::request(addr, Some(&success.token), &request_body).await;
    assert_eq!(status, 200);
    let DbResponse::Batch { response } =
        rmp_serde::from_slice::<DbResponse>(&body).expect("decode read response")
    else {
        panic!("expected batch response");
    };
    let records = &response.results.get("rd").expect("rd alias").records;
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].get_value_str("marker"),
        Some("http-login-bearer-read")
    );

    let duplicate = match login(addr, "e2e-login", PASSWORD).await {
        Err(error) => error,
        Ok(_) => panic!("expected duplicate-label rejection"),
    };
    assert_eq!(duplicate.0, 403);
    let error: ErrorBody = rmp_serde::from_slice(&duplicate.2).expect("strict duplicate error");
    assert_eq!(error.error, "token_label_exists");
    let wrong = match login(addr, "wrong-password-user", b"incorrect").await {
        Err(error) => error,
        Ok(_) => panic!("expected wrong-password rejection"),
    };
    assert_eq!(wrong.0, 401);
    let error: ErrorBody = rmp_serde::from_slice(&wrong.2).expect("strict authentication error");
    assert_eq!(error.error, "authentication_failed");

    let unknown = rmp_serde::to_vec_named(&UnknownBegin {
        protocol_version: 1,
        user: "unknown-http-login-user",
        client_nonce: serde_bytes::Bytes::new(&[7; 32]),
        binding_mode: 3,
        token: token_scope("unknown", TokenMode::ReadOnly, TokenChannels::Rest),
    })
    .expect("encode unknown begin");
    let (status, _, body) = post(addr, "/shamir/v1/auth/begin", &unknown).await;
    assert_eq!(status, 200);
    let challenge: super::common::LoginChallenge =
        rmp_serde::from_slice(&body).expect("unknown challenge");
    let handshake_id: [u8; 32] = challenge
        .handshake_id
        .as_ref()
        .try_into()
        .expect("handshake ID");
    let (status, _, body) = post(
        addr,
        "/shamir/v1/auth/finish",
        &finish_body(&handshake_id, &[0; 32]),
    )
    .await;
    assert_eq!(status, 401);
    let error: ErrorBody = rmp_serde::from_slice(&body).expect("strict unknown-user error");
    assert_eq!(error.error, "authentication_failed");

    let mut limited = false;
    for _ in 0..12 {
        let request = rmp_serde::to_vec_named(&UnknownBegin {
            protocol_version: 1,
            user: "flood-other",
            client_nonce: serde_bytes::Bytes::new(&[11; 32]),
            binding_mode: 3,
            token: token_scope("flood", TokenMode::ReadOnly, TokenChannels::Rest),
        })
        .expect("encode flood begin");
        let (status, headers, body) = post(addr, "/shamir/v1/auth/begin", &request).await;
        if status == 429 {
            let limit: LimitBody = rmp_serde::from_slice(&body).expect("strict rate-limit body");
            let retry_after = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("retry-after")
                        .then(|| value.trim().parse::<u32>().expect("numeric Retry-After"))
                })
                .expect("Retry-After header");
            assert_eq!(retry_after, limit.retry_after);
            assert_eq!(limit.error, "rate_limited");
            limited = true;
            break;
        }
        assert_eq!(
            status, 200,
            "unexpected earlier rate limit: {headers} {body:?}"
        );
    }
    assert!(limited, "login credit exhaustion should return 429");
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
}

#[derive(serde::Serialize)]
struct UnknownBegin<'a> {
    protocol_version: u8,
    user: &'a str,
    client_nonce: &'a serde_bytes::Bytes,
    binding_mode: u8,
    token: super::common::LoginScope<'a>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorBody {
    error: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitBody {
    error: String,
    retry_after: u32,
}
