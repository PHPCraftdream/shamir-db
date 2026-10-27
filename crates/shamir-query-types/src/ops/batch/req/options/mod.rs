//! Per-request typed options for [`BatchRequest`](crate::batch::BatchRequest)
//! and [`DbMessage`](crate::wire::DbMessage) — query-language v4.
//!
//! Query-language v4 replaces the free-form `Option<String>` request options
//! with strict enums (unknown values are rejected at deserialization instead
//! of silently falling back to defaults), and moves the old
//! `durability: "async_index"` string to the new orthogonal
//! [`IndexVisibility`] option.

mod decode_caps;
mod deserialization;
mod index_visibility;
mod isolation_level;
mod request_durability;
mod wire_preflight;
pub use wire_preflight::validate_request_alias_caps;

pub(crate) use decode_caps::{assert_nested_batch_caps, capped_string_map, capped_string_vec_opt};
pub(crate) use deserialization::deserialize_optional_field;

pub use index_visibility::IndexVisibility;
pub use isolation_level::IsolationLevel;
pub use request_durability::RequestDurability;
