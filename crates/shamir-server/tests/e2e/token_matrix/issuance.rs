use super::super::http_rest::common::{ping_body, request, Harness, ADMIN_PASSWORD};
use shamir_client::{Client, ClientError, ConnectOptions};
use shamir_query_builder::{batch::Batch, ddl, token_admin};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::hmac::{canonical_batch_op, compute_tag_hex, derive_session_hmac_key};
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use tokio::time::Duration;
use zeroize::Zeroizing;

pub(super) async fn connect(harness: &Harness, username: &str, password: &[u8]) -> Client {
    tokio::time::timeout(
        Duration::from_secs(15),
        Client::connect(ConnectOptions {
            addr: harness.native_addr(),
            server_name: "localhost".into(),
            username: username.into(),
            password: Zeroizing::new(password.to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: Some(Duration::from_secs(15)),
            handshake_timeout: Some(Duration::from_secs(15)),
            request_timeout: Some(Duration::from_secs(15)),
        }),
    )
    .await
    .expect("SCRAM connect timed out")
    .expect("SCRAM connect")
}

async fn shutdown(harness: Harness) {
    let report = tokio::time::timeout(Duration::from_secs(45), harness.handle.shutdown())
        .await
        .expect("server shutdown timed out");
    assert!(
        report.errors().is_empty(),
        "server shutdown errors: {:?}",
        report.errors()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_user_issues_lists_and_revokes_own_tokens() {
    let harness = Harness::launch_with_native_seed().await;
    let admin = connect(&harness, "admin", ADMIN_PASSWORD).await;
    admin
        .create_scram_user(
            "ordinary",
            Zeroizing::new("ordinary-password".into()),
            vec![],
        )
        .await
        .expect("create ordinary account");
    drop(admin);
    let user = connect(&harness, "ordinary", b"ordinary-password").await;
    let created = user
        .create_api_token(
            token_admin::create_api_token("owned")
                .ttl_secs(3600)
                .mode(ApiTokenMode::ReadOnly)
                .channels(ApiTokenChannels::Rest),
        )
        .await
        .expect("self-service create");
    let listing = user.list_api_tokens(None).await.expect("list own tokens");
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].id, created.id);
    let serialized = format!("{listing:?}");
    assert!(!serialized.contains(created.token.reveal()));
    let token_text = created.token.reveal();
    let token_bytes = token_text.as_bytes();
    let encoded_created = rmp_serde::to_vec_named(&created).expect("serialize created token");
    assert_eq!(
        encoded_created
            .windows(token_bytes.len())
            .filter(|window| *window == token_bytes)
            .count(),
        1
    );
    let encoded = rmp_serde::to_vec(&listing).expect("serialize listing");
    assert!(!String::from_utf8_lossy(&encoded).contains(token_text));
    let secret = token_text.rsplit('.').next().expect("token secret segment");
    assert!(!encoded
        .windows(secret.len())
        .any(|window| window == secret.as_bytes()));
    let (status, _, _) = request(harness.addr(), Some(token_text), &ping_body()).await;
    assert_eq!(status, 200);
    user.revoke_api_token(&created.id)
        .await
        .expect("revoke own token");
    assert!(
        user.list_api_tokens(None).await.expect("list after revoke")[0]
            .revoked_at_ns
            .is_some()
    );
    drop(user);
    let (status, _, _) = request(harness.addr(), Some(token_text), &ping_body()).await;
    assert_eq!(status, 401);
    shutdown(harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_user_cannot_delegate_or_manage_foreign_tokens() {
    let harness = Harness::launch_with_native_seed().await;
    let admin = connect(&harness, "admin", ADMIN_PASSWORD).await;
    admin
        .create_scram_user(
            "ordinary",
            Zeroizing::new("ordinary-password".into()),
            vec![],
        )
        .await
        .expect("create ordinary account");
    let delegated_id = admin
        .create_api_token(token_admin::create_api_token("foreign").for_user("ordinary"))
        .await
        .expect("admin token for ordinary user")
        .id;
    let foreign_id = admin
        .create_api_token(token_admin::create_api_token("foreign-admin"))
        .await
        .expect("admin token")
        .id;
    drop(admin);
    let user = connect(&harness, "ordinary", b"ordinary-password").await;
    let own_listing = user.list_api_tokens(None).await.expect("list own tokens");
    assert_eq!(own_listing.len(), 1);
    assert_eq!(own_listing[0].id, delegated_id);
    assert!(
        matches!(user.list_api_tokens(Some("admin")).await, Err(ClientError::Db { code, .. }) if code == "permission_denied")
    );
    assert!(
        matches!(user.revoke_api_token(&foreign_id).await, Err(ClientError::Db { code, .. }) if code == "token_not_found")
    );
    assert!(
        matches!(user.create_api_token(token_admin::create_api_token("delegate").for_user("admin")).await, Err(ClientError::Db { code, .. }) if code == "permission_denied")
    );
    drop(user);
    shutdown(harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_issuance_caps_are_enforced() {
    let harness = Harness::launch_with_native_seed_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(false),
            ..Default::default()
        },
        |config| {
            config.tokens.max_per_user = 2;
            config.tokens.max_ttl_days = 1;
            config.tokens.default_ttl_days = 1;
        },
    )
    .await;
    let admin = connect(&harness, "admin", ADMIN_PASSWORD).await;
    admin
        .create_scram_user(
            "ordinary",
            Zeroizing::new("ordinary-password".into()),
            vec![],
        )
        .await
        .expect("create ordinary account");
    drop(admin);
    let user = connect(&harness, "ordinary", b"ordinary-password").await;
    for ttl in [0, 86_401] {
        assert!(
            matches!(user.create_api_token(token_admin::create_api_token("invalid").ttl_secs(ttl)).await, Err(ClientError::Db { code, .. }) if code == "token_ttl_invalid")
        );
    }
    let first = user
        .create_api_token(token_admin::create_api_token("one-day").ttl_secs(86_400))
        .await
        .expect("one-day token");
    assert_eq!(
        first.expires_at_ns - first.created_at_ns,
        86_400_000_000_000
    );
    user.create_api_token(token_admin::create_api_token("second"))
        .await
        .expect("second token");
    assert!(
        matches!(user.create_api_token(token_admin::create_api_token("third")).await, Err(ClientError::Db { code, .. }) if code == "token_limit_reached")
    );
    drop(user);
    shutdown(harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_self_service_retains_list_and_revoke() {
    let harness = Harness::launch_with_native_seed_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(false),
            ..Default::default()
        },
        |config| config.tokens.allow_self_service = false,
    )
    .await;
    let admin = connect(&harness, "admin", ADMIN_PASSWORD).await;
    admin
        .create_scram_user(
            "ordinary",
            Zeroizing::new("ordinary-password".into()),
            vec![],
        )
        .await
        .expect("create ordinary account");
    let delegated = admin
        .create_api_token(token_admin::create_api_token("delegated").for_user("ordinary"))
        .await
        .expect("admin token for ordinary user");
    drop(admin);
    let user = connect(&harness, "ordinary", b"ordinary-password").await;
    assert!(
        matches!(user.create_api_token(token_admin::create_api_token("denied")).await, Err(ClientError::Db { code, .. }) if code == "token_self_service_disabled")
    );
    let listing = user
        .list_api_tokens(None)
        .await
        .expect("list own delegated token");
    assert!(listing.iter().any(|token| token.id == delegated.id));
    user.revoke_api_token(&delegated.id)
        .await
        .expect("revoke own delegated token");
    drop(user);
    shutdown(harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_sessions_cannot_manage_tokens_over_rest() {
    let harness = Harness::launch().await;
    let token = harness
        .token(
            "token-admin-denied",
            shamir_connect::server::token::TokenMode::ReadWrite,
        )
        .await;
    let (status, _, _) = request(harness.addr(), Some(&token), &ping_body()).await;
    assert_eq!(status, 200);

    let requests = [
        token_admin::create_api_token("denied").build(),
        token_admin::list_api_tokens().build(),
        token_admin::revoke_api_token("AAAAAAAAAAAAAAAAAAAAAA").build(),
        token_admin::revoke_user_tokens().build(),
    ];
    for query in requests {
        let body = rmp_serde::to_vec_named(&query).expect("serialize token administration request");
        let (status, _, response) = request(harness.addr(), Some(&token), &body).await;
        assert_eq!(status, 403);
        let response: shamir_query_types::wire::DbResponse =
            rmp_serde::from_slice(&response).expect("decode token administration response");
        assert!(
            matches!(response, shamir_query_types::wire::DbResponse::Error { code, .. } if code == "token_request_denied")
        );
    }

    let (status, _, _) = request(harness.addr(), Some(&token), &ping_body()).await;
    shutdown(harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_role_change_invalidates_rest_tokens() {
    for tls in [false, true] {
        let harness = Harness::launch_with_native_seed_hook(
            "127.0.0.1:0",
            shamir_server::config::HttpListenerSettings {
                tls: Some(tls),
                ..Default::default()
            },
            |_| {},
        )
        .await;
        let admin = connect(&harness, "admin", ADMIN_PASSWORD).await;
        let rest_token = admin
            .create_api_token(
                token_admin::create_api_token("owner-role-change")
                    .mode(ApiTokenMode::ReadOnly)
                    .channels(ApiTokenChannels::Rest),
            )
            .await
            .expect("create owner REST token");

        let get = format!(
            "GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            rest_token.token.reveal()
        );
        let (head, _) = super::support::send_http(&harness, tls, &get).await;
        assert_eq!(super::support::status(&head), 200);
        let unsigned = ddl::grant_role("matrix_role", "admin")
            .hmac("placeholder")
            .build()
            .expect("build placeholder grant_role");
        let BatchOp::GrantRole(mut grant) = unsigned else {
            panic!("grant_role builder returned another operation");
        };
        grant.hmac = None;
        let unsigned = BatchOp::GrantRole(grant);
        let key = derive_session_hmac_key(&admin.session_id());
        let tag = compute_tag_hex(
            &key,
            &canonical_batch_op("default", &unsigned).expect("canonical grant_role"),
        );
        let mut batch = Batch::new();
        batch.grant_role(
            "role_change",
            ddl::grant_role("matrix_role", "admin")
                .hmac(tag)
                .build()
                .expect("build signed grant_role"),
        );
        admin
            .execute("default", batch.build())
            .await
            .expect("grant owner role");

        let (head, body) = super::support::send_http(&harness, tls, &get).await;
        assert_eq!(super::support::status(&head), 401);
        assert_eq!(body, b"\x81\xa5error\xacunauthorized");
        drop(admin);
        shutdown(harness).await;
    }
}
