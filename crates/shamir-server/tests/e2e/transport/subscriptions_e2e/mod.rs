//! Live Subscriptions v1.1 — wire-level end-to-end tests.
//!
//! Drives `ShamirDbHandler::handle()` with msgpack-encoded `DbRequest`
//! batches built exclusively via the typed `shamir-query-builder` API,
//! captures server-initiated push frames through a `PushSink` connected
//! to a `tokio::sync::mpsc` channel, and decodes them as `PushEnvelope`.
//!
//! Tests cover: basic subscribe → insert → event delivery; server-side
//! filter eval suppressing non-matching events; the `subscribe!` macro;
//! multiple subscriptions in one batch; unsubscribe stops further pushes;
//! and the multi-repo guard rejects multi-source subscriptions across
//! different repos with code `multi_repo_subscriptions_not_supported`.

mod acl;
mod basics;
mod common;
mod macro_filter;
mod routing;
