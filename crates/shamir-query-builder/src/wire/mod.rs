//! Serialize any builder-produced wire DTO to JSON or msgpack.
//!
//! `build()` already yields the internal struct for the embedded path;
//! this module adds the network formats via the [`ToWire`] extension trait.
//!
//! The blanket impl means any `T: Serialize` automatically gains
//! `.to_json_value()`, `.to_json_string()`, `.to_json_string_pretty()`,
//! and `.to_msgpack()`:
//!
//! ```rust
//! use shamir_query_builder::{Query, wire::ToWire};
//!
//! let bytes = Query::from("users").build().to_msgpack().unwrap();
//! let json  = Query::from("users").build().to_json_string().unwrap();
//! ```

use serde::Serialize;

/// Encode a wire DTO (e.g. `BatchRequest`, `ReadQuery`) into the transport
/// formats ShamirDB accepts.
pub trait ToWire: Serialize {
    /// JSON `serde_json::Value` (text transports / debugging).
    fn to_json_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }

    /// Compact JSON string.
    fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Pretty-printed JSON string.
    fn to_json_string_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Msgpack with NAMED fields — matches the wire encoding the
    /// client/server use (`rmp_serde::to_vec_named`, see shamir-client).
    fn to_msgpack(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        rmp_serde::to_vec_named(self)
    }
}

/// Blanket implementation: every `Serialize` type is wire-encodable.
impl<T: Serialize + ?Sized> ToWire for T {}

#[cfg(test)]
mod tests;
