//! Client matrix over live servers. `session/`: session over the native
//! WebSocket listener, ticket resume over WebSocket (also across TCP and
//! WebSocket), session and resume over plain loopback TCP, and the refusal of
//! a non-loopback plain address. `browser/` (T9-8a): session and resume over
//! the browser-profile WebSocket listener (`tls_no_export`, binding mode
//! 0x02, `Origin` required). `token/`: token REST (`TokenHttpClient`) over
//! TLS and plain loopback HTTP listeners, token WebSocket and login.

mod browser;
mod common;
mod session;
mod token;
