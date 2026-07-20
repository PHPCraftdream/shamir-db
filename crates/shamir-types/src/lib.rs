//! Foundation types for ShamirDB — value model, identifiers, codecs,
//! and the string interner runtime that everything above this crate
//! depends on.
//!
//! This crate is intentionally storage-engine-agnostic and query-language-
//! agnostic: nothing in here knows about redb, sled, batch queries, or
//! sessions. Higher-level crates (`shamir-db` engine + query language,
//! `shamir-server`, transports) all sit on top.
//!
//! Module map mirrors the legacy in-tree layout exactly so existing
//! `crate::types::...` / `crate::codecs::...` / `crate::core::interner::...`
//! paths in `shamir-db` continue to resolve via re-exports.

pub mod access;
pub mod codecs;
pub mod core;
pub mod macros;
pub mod record_view;
pub mod secret;
pub mod types;

#[cfg(test)]
mod tests;
