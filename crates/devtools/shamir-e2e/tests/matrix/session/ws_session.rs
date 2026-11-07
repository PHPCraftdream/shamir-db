//! Session over the native WebSocket listener.

use shamir_client::Client;

use crate::common::{boot, exercise, options};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_over_websocket_runs_batches_and_serves_a_concurrent_second_client() {
    let matrix = boot().await;

    let first = Client::connect_ws(options(matrix.ws))
        .await
        .expect("connect_ws first");
    assert_ne!(first.server_pub_key_pin(), [0u8; 32], "TOFU pin captured");
    assert!(
        first.resumption_ticket().is_some(),
        "server should issue a resumption ticket"
    );
    let second = Client::connect_ws(options(matrix.ws))
        .await
        .expect("connect_ws second");

    tokio::join!(exercise(&first, "ws_one"), exercise(&second, "ws_two"));

    first.close().await;
    second.close().await;
    matrix.shutdown().await;
}
