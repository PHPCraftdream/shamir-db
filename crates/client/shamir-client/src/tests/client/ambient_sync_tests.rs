//! Client query-version negotiation and interner-epoch advertisement
//! (RV-CLIENT-1C219006): the legacy default, the v1/v2 outbound-frame
//! advertisement of `interner_epochs` (checked by decoding the real frame
//! over `Client::for_test` duplex pipes), and the advertisement predicate.

use shamir_connect::common::envelope::RequestEnvelope;
use shamir_query_builder::batch::Batch;
use shamir_query_types::wire::DbRequest;
use tokio::io::AsyncReadExt;

use crate::client::should_advertise_interner_epochs;
use crate::Client;

#[tokio::test]
async fn rv_client_1c219006_default_client_version_is_legacy() {
    let (s2c, _unused) = tokio::io::duplex(1024);
    let (reader, _write) = tokio::io::split(s2c);
    let (_read, writer) = tokio::io::split(tokio::io::duplex(1024).0);
    let client = Client::for_test(reader, writer, crate::client::LifecycleHooks::default());
    assert_eq!(client.server_query_version_for_test(), 0, "legacy default");
    drop(client);
}

#[tokio::test]
async fn rv_client_1c219006_v1_execution_does_not_create_cache_keys() {
    // Check the encoded request, not only the admission predicate.
    let (client_to_server, mut server_reader) = tokio::io::duplex(64 * 1024);
    let (_server_to_client, client_reader) = tokio::io::duplex(64 * 1024);
    let mut batch = Batch::new();
    batch.query("read", shamir_query_builder::Query::from("items"));
    let client = Client::for_test(
        client_reader,
        client_to_server,
        crate::client::LifecycleHooks::default(),
    );
    let execute = tokio::spawn(async move { client.execute("ic", batch.build()).await });
    let mut prefix = [0u8; 4];
    server_reader
        .read_exact(&mut prefix)
        .await
        .expect("frame prefix");
    let len = u32::from_be_bytes(prefix) as usize;
    assert!(len <= 64 * 1024, "bounded test frame");
    let mut frame = vec![0; len];
    server_reader
        .read_exact(&mut frame)
        .await
        .expect("frame body");
    let envelope = RequestEnvelope::from_msgpack(&frame).expect("request envelope");
    let DbRequest::Execute { batch, .. } =
        rmp_serde::from_slice(&envelope.req).expect("execute request")
    else {
        panic!("expected Execute");
    };
    assert!(
        batch.interner_epochs.is_empty(),
        "legacy execution sends no cursor"
    );
    execute.abort();
    let _ = execute.await;
}

#[tokio::test]
async fn rv_client_1c219006_v2_execution_advertises_cold_epoch() {
    // Decode the actual outbound frame to protect cold-v2 synchronization.
    let (client_to_server, mut server_reader) = tokio::io::duplex(64 * 1024);
    let (_server_to_client, client_reader) = tokio::io::duplex(64 * 1024);
    let mut batch = Batch::new();
    batch.query("read", shamir_query_builder::Query::from("items"));
    batch.query(
        "other",
        shamir_query_builder::Query::with_repo("explicit-other", "items"),
    );
    let client = Client::for_test(
        client_reader,
        client_to_server,
        crate::client::LifecycleHooks::default(),
    );
    client.set_server_query_version_for_test(2);
    let mut batch = batch.build();
    batch.interner_epochs.insert("explicit-other".into(), 42);
    let execute = tokio::spawn(async move { client.execute("ic", batch).await });
    let mut prefix = [0u8; 4];
    server_reader
        .read_exact(&mut prefix)
        .await
        .expect("frame prefix");
    let len = u32::from_be_bytes(prefix) as usize;
    assert!(len <= 64 * 1024, "bounded frame from client-under-test");
    let mut frame = vec![0; len];
    server_reader
        .read_exact(&mut frame)
        .await
        .expect("frame body");
    let envelope = RequestEnvelope::from_msgpack(&frame).expect("request envelope");
    let DbRequest::Execute { batch, .. } =
        rmp_serde::from_slice(&envelope.req).expect("execute request")
    else {
        panic!("expected Execute");
    };
    assert_eq!(batch.interner_epochs.len(), 2);
    assert_eq!(batch.interner_epochs.get("explicit-other"), Some(&42));
    assert!(batch
        .interner_epochs
        .iter()
        .any(|(repo, epoch)| repo != "explicit-other" && *epoch == 0));
    execute.abort();
    let _ = execute.await;
}

#[test]
fn rv_client_1c219006_legacy_server_skips_advertisement() {
    assert!(!should_advertise_interner_epochs(1, false));
    assert!(!should_advertise_interner_epochs(1, true));
}

#[test]
fn rv_client_1c219006_v2_cold_cache_still_advertises() {
    // The cold-v2 path is intentionally retained for synchronization.
    assert!(should_advertise_interner_epochs(2, false));
    assert!(should_advertise_interner_epochs(2, true));
}
