//! FG-5b — behavioral tests for `ShamirDbHandler::{create_cursor, fetch_next,
//! cancel_cursor}`, exercised through the wire `RequestHandler::handle`
//! entry point (mirrors `node_mode_tests.rs`'s harness style: a real
//! in-memory `ShamirDb` with an owned table, wire msgpack round-trip).
//! Declarations/re-exports only; the fixtures live in `common`.

pub mod common;
pub mod index_seek_tests;
pub mod lifecycle_permission_tests;
pub mod null_order_mode_tests;
pub mod offset_snapshot_tests;
pub mod page_size_keyset_tests;
pub mod pagination_edge_tests;
pub mod schema_null_probe_tests;
pub mod typed_value_fallback_tests;
