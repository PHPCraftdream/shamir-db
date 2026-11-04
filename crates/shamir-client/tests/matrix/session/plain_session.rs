//! Session over the plain loopback TCP listener.

use shamir_client::Client;

use crate::common::{boot, exercise, options};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_over_plain_tcp_runs_batches_and_serves_a_concurrent_second_client() {
    let matrix = boot().await;

    let first = Client::connect_plain(options(matrix.plain))
        .await
        .expect("connect_plain first");
    let second = Client::connect_plain(options(matrix.plain))
        .await
        .expect("connect_plain second");

    tokio::join!(
        exercise(&first, "plain_one"),
        exercise(&second, "plain_two")
    );

    first.close().await;
    second.close().await;
    matrix.shutdown().await;
}
