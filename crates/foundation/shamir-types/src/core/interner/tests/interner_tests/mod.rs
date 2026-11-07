//! Tests for the [`Interner`](crate::core::interner::Interner) and its key
//! types — re-exports only (CLAUDE.md §test-organisation). Logic lives in
//! topical siblings.

mod basic;
mod concurrency;
mod entries_api;
mod growth_and_gaps;
mod key_serialization;
mod user_and_touch;
