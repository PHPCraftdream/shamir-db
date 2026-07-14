//! ShamirDB connection protocol library.
//!
//! Implements `docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md` v1: transport-agnostic SCRAM-Argon2id
//! authentication with Ed25519 server identity.
//!
//! # Modules
//!
//! - [`common`] — shared types, canonical `auth_message`, crypto primitives,
//!   error types. Always available.
//! - [`client`] — SCRAM client logic (gated by `client` feature).
//! - [`server`] — SCRAM server verification + session management (gated by
//!   `server` feature).
//!
//! # Spec compliance
//!
//! Domain separation tags, byte layouts, and validation rules follow
//! `docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md` §1-§19. Wire-level test vectors live in
//! `crates/shamir-connect/test-vectors/`.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

pub mod common;

#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod client;

#[cfg(feature = "server")]
#[cfg_attr(docsrs, doc(cfg(feature = "server")))]
pub mod server;

pub use common::error::{Error, Result};
