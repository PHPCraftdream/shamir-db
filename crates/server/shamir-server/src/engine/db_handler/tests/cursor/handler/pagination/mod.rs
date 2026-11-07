//! Pagination behavior of the wire cursor handler: page-size/keyset
//! interaction, offset+snapshot semantics, and pagination edge cases.
//! Declarations/re-exports only; fixtures live in `super::common`.

pub mod offset_snapshot_tests;
pub mod page_size_keyset_tests;
pub mod pagination_edge_tests;
