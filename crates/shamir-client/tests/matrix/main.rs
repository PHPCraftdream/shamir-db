//! T9-2 client matrix over one live server with three listeners: session over
//! the native WebSocket listener, ticket resume over WebSocket (also across
//! TCP and WebSocket), session over plain loopback TCP, and the refusal of a
//! non-loopback plain address.

mod common;
mod plain_refusal;
mod plain_session;
mod ws_resume;
mod ws_session;
