//! T9-8a client matrix additions over one live server: session and ticket
//! resume over the browser-profile WebSocket listener (`profile:
//! tls_no_export`, path `/shamir/v1/browser`, binding mode 0x02, `Origin`
//! required) and ticket resume over the plain loopback TCP listener
//! (binding mode 0x00). The server fixture is shared with `tests/matrix/`.

#[path = "../matrix/common.rs"]
mod common;

mod plain_resume;
mod ws_browser_resume;
mod ws_browser_session;
