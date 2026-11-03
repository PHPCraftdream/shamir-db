use std::time::Duration;

use shamir_connect::common::crypto::derive_session_hmac_key;
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_query_builder::token_admin::create_api_token;
use shamir_query_types::auth::SecretString;
use shamir_query_types::hmac::{canonical_db_request, compute_tag_hex, verify_tag_hex};
use shamir_query_types::wire::token_admin::{
    ApiTokenChannels, ApiTokenMode, ApiTokenView, CreatedApiToken,
};
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use tokio::io::{duplex, split, AsyncRead, AsyncWrite};

use crate::client::LifecycleHooks;
use crate::{Client, ClientError};

const TIMEOUT: Duration = Duration::from_secs(5);

fn setup() -> (Client, impl AsyncWrite + Unpin, impl AsyncRead + Unpin) {
    let (client_write, server_read) = duplex(64 * 1024);
    let (server_write, client_read) = duplex(64 * 1024);
    let (client_read, _) = split(client_read);
    let (_, client_write) = split(client_write);
    (
        Client::for_test(client_read, client_write, LifecycleHooks::default()),
        server_write,
        server_read,
    )
}

async fn read_request<R: AsyncRead + Unpin>(reader: &mut R) -> (u32, DbRequest) {
    let frame = tokio::time::timeout(TIMEOUT, read_frame(reader, MAX_FRAME_SIZE_DEFAULT))
        .await
        .expect("request frame timeout")
        .expect("request frame");
    let envelope = RequestEnvelope::from_msgpack(&frame).expect("request envelope");
    assert_eq!(envelope.session_id, [0; 32]);
    (
        envelope.request_id.expect("request id"),
        rmp_serde::from_slice(&envelope.req).expect("request payload"),
    )
}

async fn answer<W: AsyncWrite + Unpin>(writer: &mut W, id: u32, response: DbResponse) {
    let payload = rmp_serde::to_vec_named(&response).expect("response payload");
    let frame = ResponseEnvelope::ok(Some(id), payload)
        .to_msgpack()
        .expect("response envelope");
    tokio::time::timeout(TIMEOUT, write_frame(writer, &frame))
        .await
        .expect("response write timeout")
        .expect("response write");
}

fn signed(request: &DbRequest) {
    let tag = match request {
        DbRequest::CreateApiToken { hmac, .. }
        | DbRequest::ListApiTokens { hmac, .. }
        | DbRequest::RevokeApiToken { hmac, .. }
        | DbRequest::RevokeUserTokens { hmac, .. } => hmac.as_deref().expect("request hmac"),
        _ => panic!("unexpected operation"),
    };
    let canonical = canonical_db_request(request).unwrap();
    let key = derive_session_hmac_key(&[0; 32]);
    assert_eq!(tag, compute_tag_hex(&key, &canonical));
    assert!(verify_tag_hex(&key, &canonical, tag));
}

fn created(token: &str) -> CreatedApiToken {
    CreatedApiToken {
        id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
        token: SecretString::new(token.to_owned()),
        label: "label".into(),
        owner: "Owner".into(),
        mode: ApiTokenMode::ReadWrite,
        channels: ApiTokenChannels::Both,
        databases: Some(vec!["db".into()]),
        created_at_ns: 17,
        expires_at_ns: 91,
    }
}

#[tokio::test]
async fn create_api_token_sends_fields_signature_and_redacts_result() {
    for target in [None, Some("Cafe\u{301}MiXeD")] {
        let (client, mut writer, mut reader) = setup();
        let task = tokio::spawn(async move {
            let mut builder = create_api_token("label")
                .ttl_secs(90)
                .mode(ApiTokenMode::ReadWrite)
                .channels(ApiTokenChannels::Both)
                .databases(["db"]);
            if let Some(user) = target {
                builder = builder.for_user(user);
            }
            client.create_api_token(builder.hmac("stale")).await
        });
        let (id, request) = read_request(&mut reader).await;
        match &request {
            DbRequest::CreateApiToken {
                label,
                ttl_secs,
                for_user,
                mode,
                channels,
                databases,
                ..
            } => {
                assert_eq!(label, "label");
                assert_eq!(*ttl_secs, Some(90));
                assert_eq!(for_user.as_deref(), target);
                assert_eq!(*mode, Some(ApiTokenMode::ReadWrite));
                assert_eq!(*channels, Some(ApiTokenChannels::Both));
                assert_eq!(databases.as_deref(), Some(["db".to_owned()].as_slice()));
            }
            other => panic!("unexpected request: {other:?}"),
        }
        signed(&request);
        let secret = "shk1.AAAAAAAAAAAAAAAAAAAAAA.".to_owned() + &"A".repeat(43);
        answer(
            &mut writer,
            id,
            DbResponse::ApiTokenCreated(created(&secret)),
        )
        .await;
        let result = tokio::time::timeout(TIMEOUT, task)
            .await
            .expect("client task timeout")
            .expect("client task")
            .expect("created result");
        assert_eq!(result.token.reveal(), secret);
        assert_eq!(result.id, "AAAAAAAAAAAAAAAAAAAAAA");
        assert_eq!(result.label, "label");
        assert_eq!(result.owner, "Owner");
        assert_eq!(result.mode, ApiTokenMode::ReadWrite);
        assert_eq!(result.channels, ApiTokenChannels::Both);
        assert_eq!(result.databases, Some(vec!["db".into()]));
        assert_eq!(result.created_at_ns, 17);
        assert_eq!(result.expires_at_ns, 91);
        assert!(!format!("{result:?}").contains(&secret));
    }
}

#[tokio::test]
async fn list_api_tokens_preserves_raw_username_and_all_metadata() {
    for target in [None, Some("Cafe\u{301}MiXeD")] {
        let (client, mut writer, mut reader) = setup();
        let task = tokio::spawn(async move { client.list_api_tokens(target).await });
        let (id, request) = read_request(&mut reader).await;
        match &request {
            DbRequest::ListApiTokens { for_user, .. } => {
                assert_eq!(for_user.as_deref(), target);
            }
            other => panic!("unexpected request: {other:?}"),
        }
        signed(&request);
        let row = ApiTokenView {
            id: "id".into(),
            label: "label".into(),
            owner: "owner".into(),
            mode: ApiTokenMode::ReadOnly,
            channels: ApiTokenChannels::Rest,
            databases: None,
            created_at_ns: 1,
            expires_at_ns: 2,
            revoked_at_ns: Some(3),
        };
        answer(
            &mut writer,
            id,
            DbResponse::ApiTokensListed {
                tokens: vec![row.clone()],
            },
        )
        .await;
        let rows = tokio::time::timeout(TIMEOUT, task)
            .await
            .expect("client task timeout")
            .expect("client task")
            .expect("list result");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, row.id);
        assert_eq!(rows[0].label, row.label);
        assert_eq!(rows[0].owner, row.owner);
        assert_eq!(rows[0].mode, row.mode);
        assert_eq!(rows[0].channels, row.channels);
        assert_eq!(rows[0].databases, row.databases);
        assert_eq!(rows[0].created_at_ns, row.created_at_ns);
        assert_eq!(rows[0].expires_at_ns, row.expires_at_ns);
        assert_eq!(rows[0].revoked_at_ns, row.revoked_at_ns);
    }
}

#[tokio::test]
async fn revoke_api_token_returns_unit_and_signs() {
    let (client, mut writer, mut reader) = setup();
    let task = tokio::spawn(async move { client.revoke_api_token("id").await });
    let (id, request) = read_request(&mut reader).await;
    match &request {
        DbRequest::RevokeApiToken { id, .. } => assert_eq!(id, "id"),
        other => panic!("unexpected request: {other:?}"),
    }
    signed(&request);
    answer(&mut writer, id, DbResponse::ApiTokenRevoked).await;
    tokio::time::timeout(TIMEOUT, task)
        .await
        .expect("client task timeout")
        .expect("client task")
        .expect("revoke result");
}

#[tokio::test]
async fn revoke_user_tokens_returns_count_and_accepts_targets() {
    for target in [None, Some("Cafe\u{301}MiXeD")] {
        let (client, mut writer, mut reader) = setup();
        let task = tokio::spawn(async move { client.revoke_user_tokens(target).await });
        let (id, request) = read_request(&mut reader).await;
        match &request {
            DbRequest::RevokeUserTokens { user, .. } => {
                assert_eq!(user.as_deref(), target);
            }
            other => panic!("unexpected request: {other:?}"),
        }
        signed(&request);
        answer(
            &mut writer,
            id,
            DbResponse::ApiUserTokensRevoked { count: u64::MAX },
        )
        .await;
        assert_eq!(
            tokio::time::timeout(TIMEOUT, task)
                .await
                .expect("client task timeout")
                .expect("client task")
                .unwrap(),
            u64::MAX
        );
    }
}

#[tokio::test]
async fn section_twelve_errors_map_to_exact_database_codes() {
    let codes = [
        "permission_denied",
        "token_not_found",
        "token_browser_session",
        "token_request_denied",
        "token_self_service_disabled",
        "read_only_replica",
        "hmac_required",
        "hmac_mismatch",
        "session_invalidated",
        "invalid_username",
        "not_found",
        "not_supported",
        "internal",
        "token_label_exists",
        "token_limit_reached",
        "token_ttl_invalid",
    ];
    for operation in 0..4 {
        for code in codes {
            let (client, mut writer, mut reader) = setup();
            let task = tokio::spawn(async move {
                match operation {
                    0 => client
                        .create_api_token(create_api_token("label"))
                        .await
                        .map(|_| ()),
                    1 => client.list_api_tokens(None).await.map(|_| ()),
                    2 => client.revoke_api_token("id").await,
                    3 => client.revoke_user_tokens(None).await.map(|_| ()),
                    _ => unreachable!(),
                }
            });
            let (id, request) = read_request(&mut reader).await;
            signed(&request);
            answer(
                &mut writer,
                id,
                DbResponse::Error {
                    code: code.into(),
                    message: "fixed".into(),
                },
            )
            .await;
            let error = tokio::time::timeout(TIMEOUT, task)
                .await
                .expect("client task timeout")
                .expect("client task")
                .expect_err("db error expected");
            assert!(matches!(&error, ClientError::Db { code: actual, .. } if actual == code));
            assert!(!format!("{error:?}{error}").contains("shk1."));
        }
    }
}

#[tokio::test]
async fn malformed_creation_tokens_are_rejected_with_fixed_diagnostic() {
    let id_mismatch = "shk1.BBBBBBBBBBBBBBBBBBBBBB.".to_owned() + &"A".repeat(43);
    let short = "shk1.AAAAAAAAAAAAAAAAAAAAAA.".to_owned() + &"A".repeat(42);
    let long = "shk1.AAAAAAAAAAAAAAAAAAAAAA.".to_owned() + &"A".repeat(44);
    let wrong_prefix = "bad1.AAAAAAAAAAAAAAAAAAAAAA.".to_owned() + &"A".repeat(43);
    let wrong_separator = "shk1.AAAAAAAAAAAAAAAAAAAAAA!".to_owned() + &"A".repeat(43);
    let unicode = format!("shk1.{}é{}", "A".repeat(21), "A".repeat(43));
    for token in [
        "not-a-token".to_owned(),
        id_mismatch,
        short,
        long,
        wrong_prefix,
        wrong_separator,
        unicode,
    ] {
        let (client, mut writer, mut reader) = setup();
        let task =
            tokio::spawn(async move { client.create_api_token(create_api_token("label")).await });
        let (id, _) = read_request(&mut reader).await;
        answer(
            &mut writer,
            id,
            DbResponse::ApiTokenCreated(created(&token)),
        )
        .await;
        let error = tokio::time::timeout(TIMEOUT, task)
            .await
            .expect("client task timeout")
            .expect("client task")
            .expect_err("malformed token");
        assert!(matches!(
            &error,
            ClientError::Protocol(message)
                if message == "malformed api token in creation response"
        ));
        assert!(!format!("{error:?}{error}").contains(&token));
    }
}

#[tokio::test]
async fn unexpected_response_kinds_are_protocol_errors_for_every_method() {
    for operation in 0..4 {
        let (client, mut writer, mut reader) = setup();
        let task = tokio::spawn(async move {
            match operation {
                0 => client
                    .create_api_token(create_api_token("label"))
                    .await
                    .map(|_| ()),
                1 => client.list_api_tokens(None).await.map(|_| ()),
                2 => client.revoke_api_token("id").await,
                _ => client.revoke_user_tokens(None).await.map(|_| ()),
            }
        });
        let (id, _) = read_request(&mut reader).await;
        let token = "shk1.AAAAAAAAAAAAAAAAAAAAAA.".to_owned() + &"A".repeat(43);
        let response = if operation == 0 {
            DbResponse::Pong
        } else {
            DbResponse::ApiTokenCreated(created(&token))
        };
        answer(&mut writer, id, response).await;
        let error = tokio::time::timeout(TIMEOUT, task)
            .await
            .expect("client task timeout")
            .expect("client task")
            .expect_err("unexpected kind");
        assert!(matches!(&error, ClientError::Protocol(_)));
        assert!(!format!("{error:?}{error}").contains(&token));
    }
}
