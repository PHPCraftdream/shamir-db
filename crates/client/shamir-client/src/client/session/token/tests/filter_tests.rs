use std::time::Duration;

use shamir_connect::server::token::{encode_token, generate_token_parts};
use shamir_query_types::batch::VisibilityWait;
use shamir_query_types::wire::cursor_id::CursorId;
use shamir_query_types::wire::{DbRequest, CURRENT_QUERY_LANG_VERSION};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

use super::super::request_filter::check_request_allowed;
use crate::builder::batch::Batch;
use crate::builder::query::Query;
use crate::{ApiToken, ClientError, TokenEndpoint, TokenHttpClient, TokenHttpOptions};

fn small_batch() -> crate::BatchRequest {
    let mut batch = Batch::new();
    batch.query("u", Query::from("users"));
    batch.build()
}

fn tx_begin() -> DbRequest {
    DbRequest::TxBegin {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "db".into(),
        repo: "repo".into(),
        isolation: None,
    }
}

fn code_of(result: Result<(), ClientError>) -> String {
    match result {
        Err(ClientError::Db { code, .. }) => code,
        other => panic!("expected Db error, got {other:?}"),
    }
}

#[test]
fn allowed_requests_pass() {
    let allowed = [
        DbRequest::Ping,
        DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "db".into(),
            batch: small_batch(),
        },
        DbRequest::WaitForVisibility {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "db".into(),
            repo: "repo".into(),
            version: 1,
            visibility_wait: VisibilityWait::Strict,
        },
        DbRequest::GetDdlOpStatus {
            db: "db".into(),
            repo: "repo".into(),
            table: "t".into(),
            op_id: "op".into(),
        },
    ];
    for req in &allowed {
        assert!(check_request_allowed(req).is_ok());
    }
}

#[test]
fn tx_and_cursor_requests_are_not_supported() {
    let denied = [
        tx_begin(),
        DbRequest::TxExecute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "db".into(),
            tx_handle: 1,
            batch: small_batch(),
        },
        DbRequest::TxCommit {
            query_version: CURRENT_QUERY_LANG_VERSION,
            visibility_wait: None,
            db: "db".into(),
            tx_handle: 1,
        },
        DbRequest::TxRollback {
            db: "db".into(),
            tx_handle: 1,
        },
        DbRequest::CreateCursor {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "db".into(),
            query: Query::from("users").build(),
            page_size: 10,
        },
        DbRequest::FetchNext {
            cursor_id: CursorId(1),
            page_size: None,
        },
        DbRequest::CancelCursor {
            cursor_id: CursorId(1),
        },
    ];
    for req in &denied {
        assert_eq!(
            code_of(check_request_allowed(req)),
            "not_supported_over_rest"
        );
    }
}

#[test]
fn admin_requests_are_denied() {
    let req = DbRequest::ListApiTokens {
        for_user: None,
        hmac: None,
    };
    assert_eq!(code_of(check_request_allowed(&req)), "token_request_denied");
}

#[tokio::test]
async fn refused_request_sends_nothing() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buf = [0u8; 1024];
        tokio::time::timeout(Duration::from_millis(300), socket.read(&mut buf))
            .await
            .map(|read| read.unwrap_or(0))
            .unwrap_or(0)
    });

    let (id, secret) = generate_token_parts().expect("token parts");
    let token = ApiToken::new(encode_token(&id, &secret).to_string()).expect("token");
    let endpoint = TokenEndpoint::plain_loopback(addr).expect("endpoint");
    let client = TokenHttpClient::connect(TokenHttpOptions::new(endpoint, token))
        .await
        .expect("connect");

    let err = client.request(&tx_begin()).await.expect_err("refused");
    assert!(matches!(err, ClientError::Db { ref code, .. } if code == "not_supported_over_rest"));

    let received = server.await.expect("server task");
    assert_eq!(received, 0);
    client.close().await;
}
