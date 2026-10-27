//! Serialize any builder-produced wire DTO to msgpack or a [`QueryValue`] tree.
//!
//! `build()` already yields the internal struct for the embedded path;
//! this module adds the network formats via the [`ToWire`] extension trait.
//!
//! The blanket impl means any `T: Serialize` automatically gains
//! `.to_query_value()` and `.to_msgpack()`:
//!
//! ```rust
//! use shamir_query_builder::{Query, wire::ToWire};
//!
//! // Primary wire encoding — msgpack with named fields:
//! let bytes = Query::from("users").build().to_msgpack().unwrap();
//!
//! // Programmatic inspection:
//! let qv = Query::from("users").build().to_query_value().unwrap();
//! ```
//!
//! [`QueryValue`]: shamir_types::types::value::QueryValue

mod error;
mod to_wire;

pub mod response;

pub use error::WireError;
pub use to_wire::ToWire;

#[cfg(test)]
mod tests;
