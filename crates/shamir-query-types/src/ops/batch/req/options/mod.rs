//! Per-request typed options for [`BatchRequest`](crate::batch::BatchRequest)
//! and [`DbMessage`](crate::wire::DbMessage) — query-language v4.
//!
//! Query-language v4 replaces the free-form `Option<String>` request options
//! with strict enums (unknown values are rejected at deserialization instead
//! of silently falling back to defaults), and moves the old
//! `durability: "async_index"` string to the new orthogonal
//! [`IndexVisibility`] option.

mod deserialization;
mod index_visibility;
mod isolation_level;
mod request_durability;

pub(crate) use deserialization::deserialize_optional_field;

pub use index_visibility::IndexVisibility;
pub use isolation_level::IsolationLevel;
pub use request_durability::RequestDurability;
