//! Ticket resume over WebSocket, and across TCP and WebSocket.

use zeroize::Zeroizing;

use shamir_client::{Client, SecureResumeOptions};

use crate::common::{boot, exercise, options, resume_options, ticket_of};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_over_websocket_after_a_websocket_session() {
    let matrix = boot().await;

    let first = Client::connect_ws(options(matrix.ws))
        .await
        .expect("connect_ws");
    let resumed = Client::resume_ws(resume_options(matrix.ws, &first))
        .await
        .expect("resume_ws");
    assert_eq!(resumed.server_pub_key_pin(), first.server_pub_key_pin());
    exercise(&resumed, "resumed_ws").await;

    resumed.close().await;
    first.close().await;
    matrix.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_secure_over_websocket() {
    let matrix = boot().await;

    let first = Client::connect_ws(options(matrix.ws))
        .await
        .expect("connect_ws");
    let resumed = Client::resume_ws_secure(SecureResumeOptions {
        addr: matrix.ws,
        server_name: "localhost".into(),
        ticket: Zeroizing::new(ticket_of(&first)),
        pinned_hash: first.server_pub_key_pin(),
    })
    .await
    .expect("resume_ws_secure");
    resumed.ping().await.expect("ping");

    resumed.close().await;
    first.close().await;
    matrix.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticket_from_tcp_resumes_over_websocket() {
    let matrix = boot().await;

    let tcp_client = Client::connect(options(matrix.tcp))
        .await
        .expect("connect tcp");
    let resumed = Client::resume_ws(resume_options(matrix.ws, &tcp_client))
        .await
        .expect("resume_ws");
    resumed.ping().await.expect("ping");

    resumed.close().await;
    tcp_client.close().await;
    matrix.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticket_from_websocket_resumes_over_tcp() {
    let matrix = boot().await;

    let ws_client = Client::connect_ws(options(matrix.ws))
        .await
        .expect("connect_ws");
    let resumed = Client::resume(resume_options(matrix.tcp, &ws_client))
        .await
        .expect("resume tcp");
    resumed.ping().await.expect("ping");

    resumed.close().await;
    ws_client.close().await;
    matrix.shutdown().await;
}
