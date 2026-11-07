//! Ticket resume over the browser-profile WebSocket listener (binding 0x02).

use shamir_client::Client;

use crate::common::{boot, browser_resume_options, browser_session_options, exercise, options};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_over_browser_ws_after_a_browser_ws_session() {
    let matrix = boot().await;

    let first = Client::connect_ws_browser(browser_session_options(&matrix))
        .await
        .expect("connect_ws_browser");
    let resumed = Client::resume_ws_browser(browser_resume_options(&matrix, &first))
        .await
        .expect("resume_ws_browser");
    assert_eq!(resumed.server_pub_key_pin(), first.server_pub_key_pin());
    exercise(&resumed, "resumed_browser_ws").await;

    resumed.close().await;
    first.close().await;
    matrix.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_ticket_is_refused_over_browser_ws() {
    let matrix = boot().await;

    let first = Client::connect_ws_browser(browser_session_options(&matrix))
        .await
        .expect("connect_ws_browser");
    let mut forged = browser_resume_options(&matrix, &first);
    forged.inner.ticket = vec![0x51; 64];
    let err = match Client::resume_ws_browser(forged).await {
        Err(e) => e,
        Ok(_) => panic!("forged ticket must be refused"),
    };
    assert!(
        matches!(
            err,
            shamir_client::ClientError::Transport { .. }
                | shamir_client::ClientError::ConnectionClosed(_)
        ),
        "{err}"
    );

    first.close().await;
    matrix.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_ws_ticket_is_refused_over_browser_ws() {
    let matrix = boot().await;

    let native = Client::connect_ws(options(matrix.ws))
        .await
        .expect("connect_ws");
    // Anti-downgrade: a ticket issued under the TLS exporter (strength 2) cannot resume under the
    // browser profile (strength 1). The server answers a generic auth failure and closes.
    let err = match Client::resume_ws_browser(browser_resume_options(&matrix, &native)).await {
        Err(e) => e,
        Ok(_) => panic!("resume downgrade native -> browser profile must be refused"),
    };
    assert!(
        matches!(
            err,
            shamir_client::ClientError::Transport { .. }
                | shamir_client::ClientError::ConnectionClosed(_)
        ),
        "{err}"
    );

    native.close().await;
    matrix.shutdown().await;
}
