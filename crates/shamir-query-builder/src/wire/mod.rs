//! Serialize any builder-produced wire DTO to msgpack or JSON.
//!
//! `build()` already yields the internal struct for the embedded path;
//! this module adds the network formats via the [`ToWire`] extension trait.
//!
//! The blanket impl means any `T: Serialize` automatically gains
//! `.to_query_value()`, `.to_msgpack()`, `.to_json_string()`, and
//! `.to_json_string_pretty()`:
//!
//! ```rust
//! use shamir_query_builder::{Query, wire::ToWire};
//!
//! // Primary wire encoding — msgpack with named fields:
//! let bytes = Query::from("users").build().to_msgpack().unwrap();
//!
//! // Debug / human-readable:
//! let json  = Query::from("users").build().to_json_string().unwrap();
//! ```

use serde::Serialize;
use shamir_types::types::value::QueryValue;

/// Encode a wire DTO (e.g. `BatchRequest`, `ReadQuery`) into the transport
/// formats ShamirDB accepts.
pub trait ToWire: Serialize {
    /// Encode as a [`QueryValue`] via a msgpack round-trip.
    ///
    /// This is the primary programmatic wire accessor: it uses the same
    /// msgpack codec as the client/server without going through
    /// `serde_json::Value`.  Panics only on an encoding bug (a type that
    /// is `Serialize` but not msgpack-serialisable is a programmer error).
    fn to_query_value(&self) -> Result<QueryValue, rmp_serde::encode::Error> {
        let bytes = rmp_serde::to_vec_named(self)?;
        // Decoding the freshly-encoded bytes into QueryValue is infallible in
        // practice (every Serialize type that encodes to valid msgpack can be
        // decoded into QueryValue).  Map the decode error into the encode
        // Error::Syntax variant so the return type stays homogeneous.
        rmp_serde::from_slice(&bytes).map_err(|e| rmp_serde::encode::Error::Syntax(e.to_string()))
    }

    /// Msgpack with NAMED fields — matches the wire encoding the
    /// client/server use (`rmp_serde::to_vec_named`, see shamir-client).
    fn to_msgpack(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        rmp_serde::to_vec_named(self)
    }

    /// Compact JSON string (debug / human-readable transport).
    ///
    /// Not used on the primary msgpack wire path; intended for logging,
    /// debugging, and text-based transports.
    fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Pretty-printed JSON string (debug / human-readable).
    ///
    /// Not used on the primary msgpack wire path; intended for logging and
    /// diagnostics.
    fn to_json_string_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// JSON `serde_json::Value` (debug / text transport).
    ///
    /// Prefer [`to_query_value`](Self::to_query_value) for programmatic
    /// access — it avoids the `serde_json::Value` intermediate.  This
    /// method is retained for tests that need structural JSON comparison.
    fn to_json_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}

/// Blanket implementation: every `Serialize` type is wire-encodable.
impl<T: Serialize + ?Sized> ToWire for T {}

#[cfg(test)]
mod tests;
