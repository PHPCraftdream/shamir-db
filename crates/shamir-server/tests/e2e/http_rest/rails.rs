use shamir_connect::server::token::TokenMode;
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

use super::common::{request, Harness};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_remote_plaintext_opt_in_allows_executor_after_write_rail() {
    let mut settings = shamir_server::config::HttpListenerSettings {
        tls: Some(false),
        allow_plaintext_remote: true,
        ..Default::default()
    };
    let addr = "0.0.0.0:0";
    let harness = Harness::launch_with(addr, settings.clone()).await;
    let token = harness
        .token("remote-write-rail", TokenMode::ReadWrite)
        .await;
    let write_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = shamir_query_builder::batch::Batch::new();
            batch.insert(
                "insert",
                shamir_query_builder::write::insert("items").row(shamir_query_builder::doc! {
                    "name" => "remote-rail"
                }),
            );
            batch.build()
        },
    };
    let write_body = rmp_serde::to_vec_named(&write_request).expect("encode write");
    let (status, _, body) = request(harness.addr(), Some(&token), &write_body).await;
    assert_eq!(status, 403);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&body).expect("decode remote write denial"),
        DbResponse::Error { code, .. } if code == "token_plaintext_write_denied"
    ));
    let report = harness.handle.shutdown().await;
    assert!(report.is_clean(), "shutdown report: {report:?}");

    settings.allow_plaintext_write = true;
    let harness = Harness::launch_with(addr, settings).await;
    let token = harness
        .token("remote-write-enabled", TokenMode::ReadWrite)
        .await;
    let write_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = shamir_query_builder::batch::Batch::new();
            batch.insert(
                "insert",
                shamir_query_builder::write::insert("items").row(shamir_query_builder::doc! {
                    "name" => "remote-opt-in"
                }),
            );
            batch.build()
        },
    };
    let write_body = rmp_serde::to_vec_named(&write_request).expect("encode opted-in write");
    let (status, _, body) = request(harness.addr(), Some(&token), &write_body).await;
    assert_eq!(status, 200);
    let response =
        rmp_serde::from_slice::<DbResponse>(&body).expect("decode opted-in write response");
    assert!(
        matches!(
            response,
            DbResponse::Error { code, message }
                if code != "token_plaintext_write_denied"
                    && code != "unsupported_query_version"
                    && code != "application_error"
                    && message.to_ascii_lowercase().contains("table")
        ),
        "opted-in write should reach execution and report the missing table"
    );
    let report = harness.handle.shutdown().await;
    assert!(report.is_clean(), "shutdown report: {report:?}");
}
