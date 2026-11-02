use shamir_connect::server::token::TokenMode;
use shamir_query_builder::{batch::Batch, ddl, Query};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use zeroize::Zeroizing;

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
    assert!(report.errors().is_empty(), "shutdown report: {report:?}");

    settings.allow_plaintext_write = true;
    let harness = Harness::launch_with_native_seed_settings(addr, settings).await;
    let client = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(super::common::ADMIN_PASSWORD.to_vec()),
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
    drop(client);
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
    let response = rmp_serde::from_slice::<DbResponse>(&body).expect("decode opted-in write");
    let DbResponse::Batch { response } = response else {
        panic!("expected successful insert batch, got {response:?}");
    };
    assert!(response.results.contains_key("insert"));
    let read_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.query("read", Query::from("items"));
            batch.build()
        },
    };
    let read_body = rmp_serde::to_vec_named(&read_request).expect("encode opted-in read");
    let (status, _, body) = request(harness.addr(), Some(&token), &read_body).await;
    assert_eq!(status, 200);
    let response = rmp_serde::from_slice::<DbResponse>(&body).expect("decode opted-in read");
    let DbResponse::Batch { response } = response else {
        panic!("expected successful read batch, got {response:?}");
    };
    let records = &response.results.get("read").expect("read alias").records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get_value_str("name"), Some("remote-opt-in"));
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "shutdown report: {report:?}");
}
