//! Token-authenticated REST client against a live HTTP listener.

use shamir_client::{
    ApiToken, ClientError, DbRequest, TokenHttpClient, TokenHttpOptions, CURRENT_QUERY_LANG_VERSION,
};

use super::common::{boot, create_table_batch, read_batch, seed_batch, write_batch, TokenFixture};

async fn seed_items(client: &TokenHttpClient) {
    client
        .execute("default", create_table_batch())
        .await
        .expect("create table");
    client
        .execute("default", seed_batch())
        .await
        .expect("insert row");
}

async fn assert_seeded_read(client: &TokenHttpClient) {
    let resp = client
        .execute("default", read_batch())
        .await
        .expect("read items");
    let rd = resp.results.get("rd").expect("rd alias");
    assert_eq!(rd.records.len(), 1);
    assert_eq!(rd.records[0].get_value_str("marker"), Some("token-rest"));
}

fn assert_clean(fixture: &TokenFixture, err: &ClientError) {
    let text = format!("{err} {err:?}");
    assert!(!text.contains(&fixture.read_write));
    assert!(!text.contains(&fixture.read_only));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ping_over_tls() {
    let fixture = boot().await;
    let client = fixture.connect_tls(&fixture.read_write).await;
    client.ping().await.expect("ping");
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_execute_over_tls_creates_writes_and_reads() {
    let fixture = boot().await;
    let client = fixture.connect_tls(&fixture.read_write).await;
    seed_items(&client).await;
    assert_seeded_read(&client).await;
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_unknown_secret_is_unauthorized() {
    let fixture = boot().await;
    let bad = ApiToken::new(fixture.unknown_secret_token()).expect("well-formed token");
    let client = TokenHttpClient::connect(TokenHttpOptions::new(fixture.tls_endpoint(), bad))
        .await
        .expect("connect");
    let err = client.ping().await.expect_err("unknown token must fail");
    assert_clean(&fixture, &err);
    let ClientError::Http { status, code, .. } = err else {
        panic!("expected Http error, got {err:?}");
    };
    assert_eq!(status, 401);
    assert_eq!(code.as_deref(), Some("unauthorized"));
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_read_only_write_is_denied_but_read_works() {
    let fixture = boot().await;
    let writer = fixture.connect_tls(&fixture.read_write).await;
    seed_items(&writer).await;

    let reader = fixture.connect_tls(&fixture.read_only).await;
    let err = reader
        .execute("default", write_batch())
        .await
        .expect_err("read-only write must fail");
    let ClientError::Db { code, .. } = err else {
        panic!("expected Db error, got {err:?}");
    };
    assert_eq!(code, "token_read_only");
    assert_seeded_read(&reader).await;

    reader.close().await;
    writer.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_transaction_request_is_refused_locally() {
    let fixture = boot().await;
    let client = fixture.connect_tls(&fixture.read_write).await;
    let err = client
        .request(&DbRequest::TxBegin {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "default".into(),
            repo: "main".into(),
            isolation: None,
        })
        .await
        .expect_err("transactions are not supported over REST");
    let ClientError::Db { code, .. } = err else {
        panic!("expected Db error, got {err:?}");
    };
    assert_eq!(code, "not_supported_over_rest");
    client.ping().await.expect("client still usable");
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_plain_loopback_listener_serves_ping_and_execute() {
    let fixture = boot().await;
    let client = fixture.connect_plain(&fixture.read_write).await;
    client.ping().await.expect("ping");
    seed_items(&client).await;
    assert_seeded_read(&client).await;
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_tls_rejects_self_signed_server_with_default_roots() {
    let fixture = boot().await;
    let token = ApiToken::new(fixture.read_write.clone()).expect("well-formed token");
    let options = TokenHttpOptions::new(fixture.untrusted_endpoint(), token);
    let err = match TokenHttpClient::connect(options).await {
        Err(err) => err,
        Ok(client) => client
            .ping()
            .await
            .expect_err("untrusted certificate must fail"),
    };
    assert_clean(&fixture, &err);
    fixture.shutdown().await;
}
