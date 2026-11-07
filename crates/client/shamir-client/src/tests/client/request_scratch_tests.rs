//! Zeroization of the framed request scratch slot (RV-CLIENT-150F9230):
//! after a secret-bearing request the thread-local scratch must hold no
//! residue. Runs on a current-thread runtime so the request and its peer
//! share one thread-local slot, over `Client::for_test` duplex pipes.

use tokio::io::duplex;
use zeroize::Zeroizing;

use crate::Client;

#[tokio::test(flavor = "current_thread")]
async fn rv_client_150f9230_request_scratch_empty_after_secret_send() {
    // A current-thread runtime guarantees this request and its server peer use
    // the same thread-local scratch buffer.
    let (client_write, server_read) = duplex(64 * 1024);
    let (client_read, server_write) = duplex(64);
    let client = Client::for_test(
        client_read,
        client_write,
        crate::client::LifecycleHooks::default(),
    );
    let send = client.create_scram_user("u", Zeroizing::new("pw-150f9230-secret".into()), vec![]);
    let mut receive_read = server_read;
    let receive = async move {
        let frame = shamir_transport_tcp::framing::read_frame(
            &mut receive_read,
            shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT,
        )
        .await
        .expect("request frame");
        assert!(frame
            .windows(b"pw-150f9230-secret".len())
            .any(|w| w == b"pw-150f9230-secret"));
        drop(receive_read);
        drop(server_write);
    };
    let (result, ()) = tokio::join!(send, receive);
    assert!(matches!(
        result,
        Err(crate::ClientError::ConnectionClosed(_))
    ));
    assert_eq!(client.request_scratch_len_for_test(), 0);
    client.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn rv_client_150f9230_request_scratch_capacity_wiped_after_secret_send() {
    // A current-thread runtime guarantees this request and its server peer use
    // the same thread-local scratch buffer.
    let (client_write, server_read) = duplex(64 * 1024);
    let (client_read, server_write) = duplex(64);
    let client = Client::for_test(
        client_read,
        client_write,
        crate::client::LifecycleHooks::default(),
    );
    let capacity = client.fill_request_scratch_capacity_for_test();
    let send = client.create_scram_user("u", Zeroizing::new("pw-150f9230-secret".into()), vec![]);
    let mut receive_read = server_read;
    let receive = async move {
        let frame = shamir_transport_tcp::framing::read_frame(
            &mut receive_read,
            shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT,
        )
        .await
        .expect("request frame");
        assert!(frame
            .windows(b"pw-150f9230-secret".len())
            .any(|w| w == b"pw-150f9230-secret"));
        drop(receive_read);
        drop(server_write);
    };
    let (result, ()) = tokio::join!(send, receive);
    assert!(matches!(
        result,
        Err(crate::ClientError::ConnectionClosed(_))
    ));
    // SAFETY: on this current-thread runtime the fill helper initialized every
    // capacity byte. Roundtrip may grow the Vec, but the reader checks capacity
    // against its original value before reading; Vec writes grow only past capacity.
    let scratch = unsafe { client.request_scratch_capacity_bytes_for_test(capacity) };
    assert!(scratch.iter().all(|&byte| byte == 0));
    assert!(!scratch
        .windows(b"pw-150f9230-secret".len())
        .any(|window| window == b"pw-150f9230-secret"));
    client.close().await;
}
