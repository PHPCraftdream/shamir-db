//! Session over the browser-profile WebSocket listener (binding mode 0x02).

use shamir_client::Client;

use crate::common::{boot, browser_session_options, exercise};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_over_browser_ws_runs_a_batch() {
    let matrix = boot().await;

    let client = Client::connect_ws_browser(browser_session_options(&matrix))
        .await
        .expect("connect_ws_browser");
    assert_ne!(client.server_pub_key_pin(), [0u8; 32], "TOFU pin captured");
    assert!(
        client.resumption_ticket().is_some(),
        "server should issue a resumption ticket"
    );
    exercise(&client, "browser_ws").await;

    client.close().await;
    matrix.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_ws_upgrade_is_refused_without_and_with_a_wrong_origin() {
    let matrix = boot().await;

    // Empty origin string = header omitted: the listener answers 400
    // ("origin required") and the client reports a handshake error, no panic.
    let no_origin = browser_session_options(&matrix).with_origin("");
    let err = match Client::connect_ws_browser(no_origin).await {
        Err(e) => e,
        Ok(_) => panic!("missing Origin must be refused"),
    };
    assert!(err.to_string().contains("websocket upgrade"), "{err}");

    // An origin outside the allowlist: 403, again a clean handshake error.
    let wrong = browser_session_options(&matrix).with_origin("https://evil.example");
    let err = match Client::connect_ws_browser(wrong).await {
        Err(e) => e,
        Ok(_) => panic!("wrong Origin must be refused"),
    };
    assert!(err.to_string().contains("websocket upgrade"), "{err}");

    matrix.shutdown().await;
}
