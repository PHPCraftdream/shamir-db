//! `Debug` redaction of the browser-profile session options: neither the
//! password nor the bearer ticket may appear in the output.

use std::net::SocketAddr;

use zeroize::Zeroizing;

use crate::client::session::{
    BrowserResumeOptions, BrowserWsOptions, ConnectOptions, ResumeOptions,
};
use crate::client::transport::token::TokenTrust;

#[test]
fn browser_ws_options_debug_hides_the_password() {
    let opts = BrowserWsOptions::new(
        ConnectOptions {
            addr: SocketAddr::from(([127, 0, 0, 1], 3742)),
            server_name: "localhost".into(),
            username: "admin".into(),
            password: Zeroizing::new(b"correct horse battery staple".to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: None,
            handshake_timeout: None,
            request_timeout: None,
        },
        "https://shamir.test",
        TokenTrust::webpki_roots(),
    );
    let text = format!("{opts:?}");
    assert!(text.contains("BrowserWsOptions"), "{text}");
    assert!(text.contains("https://shamir.test"), "{text}");
    assert!(!text.contains("correct horse"), "{text}");
    assert!(!text.to_lowercase().contains("password"), "{text}");
}

#[test]
fn browser_resume_options_debug_hides_the_ticket() {
    let opts = BrowserResumeOptions::new(
        ResumeOptions {
            addr: SocketAddr::from(([127, 0, 0, 1], 3742)),
            server_name: "localhost".into(),
            ticket: b"ticket-bytes-secret-0123456789".to_vec(),
            pinned_hash: [7u8; 32],
        },
        "https://shamir.test",
        TokenTrust::webpki_roots(),
    );
    let text = format!("{opts:?}");
    assert!(text.contains("BrowserResumeOptions"), "{text}");
    assert!(!text.contains("ticket-bytes-secret"), "{text}");
    assert!(!text.to_lowercase().contains("ticket"), "{text}");
}
