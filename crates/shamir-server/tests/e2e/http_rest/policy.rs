use shamir_connect::server::token::TokenMode;
use shamir_query_builder::Query;
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use shamir_server::tokens::NewToken;

use super::common::{ping_body, request, Harness};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_scope_denial_and_read_only_write_return_forbidden() {
    let harness = Harness::launch().await;
    let scoped = harness
        .handle
        .token_service
        .create(NewToken {
            owner_user_id: harness.owner,
            owner_roles: harness.owner_roles.clone(),
            label: "scope-only".into(),
            ttl_secs: Some(3600),
            mode: TokenMode::ReadOnly,
            channels: shamir_connect::server::token::TokenChannels::Both,
            databases: Some(vec!["elsewhere".into()]),
        })
        .await
        .expect("create scoped token")
        .token
        .to_string();
    let scope_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = shamir_query_builder::batch::Batch::new();
            batch.query("items", Query::from("items"));
            batch.build()
        },
    };
    let scope_body = rmp_serde::to_vec_named(&scope_request).expect("encode scope request");
    let (scope_status, _, scope_response) =
        request(harness.addr(), Some(&scoped), &scope_body).await;
    assert_eq!(scope_status, 403);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&scope_response).expect("decode scope denial"),
        DbResponse::Error { code, .. } if code == "token_database_denied"
    ));

    let readonly = harness.token("readonly-write", TokenMode::ReadOnly).await;
    let write_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = shamir_query_builder::batch::Batch::new();
            batch.insert(
                "insert",
                shamir_query_builder::write::insert("items").row(shamir_query_builder::doc! {
                    "name" => "readonly"
                }),
            );
            batch.build()
        },
    };
    let encoded = rmp_serde::to_vec_named(&write_request).expect("encode write");
    let (write_status, _, body) = request(harness.addr(), Some(&readonly), &encoded).await;
    assert_eq!(write_status, 403);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&body).expect("decode read-only denial"),
        DbResponse::Error { code, .. } if code == "token_read_only"
    ));
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "shutdown report: {report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_failure_flood_is_rate_limited_with_retry_after() {
    let harness = Harness::launch().await;
    let mut saw_limit = false;
    for _ in 0..32 {
        let (status, headers, _) = request(harness.addr(), Some("definitely-invalid"), &[]).await;
        if status == 429 {
            assert!(headers.to_ascii_lowercase().contains("retry-after:"));
            saw_limit = true;
            break;
        }
        assert_eq!(status, 401);
    }
    assert!(saw_limit, "failure flood should be rate limited");
    let valid = harness
        .token("valid-after-flood", TokenMode::ReadOnly)
        .await;
    let (status, _, _) = request(harness.addr(), Some(&valid), &ping_body()).await;
    assert_eq!(
        status, 200,
        "valid token should not inherit failure rate limit"
    );
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "shutdown report: {report:?}");
}
