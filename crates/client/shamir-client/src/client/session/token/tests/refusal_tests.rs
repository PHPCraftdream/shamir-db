use std::sync::Arc;
use std::time::Duration;

use shamir_query_types::wire::repl::ReplRequest;
use tokio::io::{duplex, AsyncReadExt, DuplexStream};
use zeroize::Zeroizing;

use crate::builder::batch::Batch;
use crate::builder::query::Query;
use crate::client::LifecycleHooks;
use crate::{AuthKind, Client, ClientError};

struct Harness {
    client: Client,
    from_client: DuplexStream,
    _to_client: DuplexStream,
}

fn harness(auth: AuthKind) -> Harness {
    let (client_to_server, from_client) = duplex(64 * 1024);
    let (_to_client, client_reader) = duplex(64 * 1024);
    let client = Client::for_test_with_auth(
        client_reader,
        client_to_server,
        LifecycleHooks::default(),
        auth,
    );
    Harness {
        client,
        from_client,
        _to_client,
    }
}

fn assert_denied(name: &str, result: Result<(), ClientError>) {
    match result {
        Err(ClientError::Db { code, .. }) => {
            assert_eq!(code, "token_request_denied", "{name}");
        }
        other => panic!("{name}: expected token_request_denied, got {other:?}"),
    }
}

#[tokio::test]
async fn auth_kind_reflects_constructor() {
    let scram = harness(AuthKind::Scram);
    assert_eq!(scram.client.auth_kind(), AuthKind::Scram);
    let (c2s, _server_in) = duplex(1024);
    let (_server_out, reader) = duplex(1024);
    let default = Client::for_test(reader, c2s, LifecycleHooks::default());
    assert_eq!(default.auth_kind(), AuthKind::Scram);
    let token = harness(AuthKind::Token);
    assert_eq!(token.client.auth_kind(), AuthKind::Token);
}

#[tokio::test]
async fn token_session_refuses_admin_operations_without_writing() {
    // A regression (a call that writes and waits for an answer) must fail, not hang.
    tokio::time::timeout(Duration::from_secs(10), refuse_all_admin_operations())
        .await
        .expect("refusals must not wait for a server response");
}

async fn refuse_all_admin_operations() {
    let Harness {
        client,
        mut from_client,
        _to_client,
    } = harness(AuthKind::Token);

    assert_denied(
        "create_api_token",
        client
            .create_api_token(shamir_query_builder::token_admin::create_api_token("label"))
            .await
            .map(|_| ()),
    );
    assert_denied(
        "list_api_tokens",
        client.list_api_tokens(None).await.map(|_| ()),
    );
    assert_denied("revoke_api_token", client.revoke_api_token("some-id").await);
    assert_denied(
        "revoke_user_tokens",
        client.revoke_user_tokens(Some("alice")).await.map(|_| ()),
    );
    assert_denied(
        "create_scram_user",
        client
            .create_scram_user(
                "bob",
                Zeroizing::new("hunter2-password".to_string()),
                vec!["reader".to_string()],
            )
            .await
            .map(|_| ()),
    );
    assert_denied("set_replicator", client.set_replicator("bob", true).await);
    assert_denied(
        "repl",
        client
            .repl(ReplRequest::Hello {
                proto_ver: 1,
                node_id: "node".to_string(),
                capabilities: None,
            })
            .await
            .map(|_| ()),
    );

    drop(client);
    let mut received = Vec::new();
    from_client
        .read_to_end(&mut received)
        .await
        .expect("read client->server side to EOF");
    assert!(
        received.is_empty(),
        "refused calls wrote {} bytes",
        received.len()
    );
}

#[tokio::test]
async fn token_session_does_not_refuse_ping_or_execute() {
    let Harness {
        client,
        mut from_client,
        _to_client,
    } = harness(AuthKind::Token);
    let client = Arc::new(client);

    let pinger = Arc::clone(&client);
    let ping = tokio::spawn(async move { pinger.ping().await });
    let mut prefix = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), from_client.read_exact(&mut prefix))
        .await
        .expect("ping must reach the wire")
        .expect("ping frame prefix");
    let len = u32::from_be_bytes(prefix) as usize;
    assert!(len > 0);
    let mut payload = vec![0u8; len];
    from_client
        .read_exact(&mut payload)
        .await
        .expect("ping frame payload");
    ping.abort();
    let _ = ping.await;

    let mut batch = Batch::new();
    batch.query("read", Query::from("items"));
    let batch = batch.build();
    let executor = Arc::clone(&client);
    let execute = tokio::spawn(async move { executor.execute("db", batch).await });
    let mut buf = [0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(5), from_client.read(&mut buf))
        .await
        .expect("execute must reach the wire")
        .expect("execute frame bytes");
    assert!(n > 0);
    execute.abort();
    let _ = execute.await;
}
