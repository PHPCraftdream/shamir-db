//! `connect_plain` refuses non-loopback addresses before opening any socket.

use std::net::SocketAddr;
use std::time::Duration;

use shamir_client::{Client, ClientError};

use crate::common::options;

#[tokio::test]
async fn connect_plain_refuses_non_loopback_addresses_before_any_socket() {
    for text in ["192.0.2.1:9", "[2001:db8::1]:9"] {
        let addr: SocketAddr = text.parse().expect("socket addr");
        let result =
            tokio::time::timeout(Duration::from_secs(1), Client::connect_plain(options(addr)))
                .await
                .expect("refusal is immediate");
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("non-loopback plain TCP must be refused"),
        };
        assert!(matches!(err, ClientError::Handshake { .. }), "{err}");
        assert!(
            err.to_string().contains("plain TCP is loopback-only"),
            "{err}"
        );
    }
}
