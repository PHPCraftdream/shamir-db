//! u64 field deserialization for the `changes_since` wire forms.
//!
//! The server serializes the response through `to_qv`, so every integer
//! arrives as a `QueryValue`: `u64 <= i64::MAX` becomes `Int` (msgpack
//! int), anything above becomes `Big` (msgpack string with the decimal
//! digits). These adapters accept both, so the u64 DTO fields decode
//! losslessly whatever side of `i64::MAX` the value is on. Serialization
//! stays the derived u64 form.

use serde::de::{Deserializer, Error as DeError, Visitor};
use std::fmt;

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = u64;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("u64 as msgpack uint/int or a decimal string")
        }
        fn visit_u64<E: DeError>(self, v: u64) -> Result<u64, E> {
            Ok(v)
        }
        fn visit_i64<E: DeError>(self, v: i64) -> Result<u64, E> {
            u64::try_from(v).map_err(DeError::custom)
        }
        fn visit_str<E: DeError>(self, v: &str) -> Result<u64, E> {
            v.parse::<u64>().map_err(DeError::custom)
        }
    }
    d.deserialize_any(V)
}

pub(crate) fn deserialize_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    struct OV;
    impl<'de> Visitor<'de> for OV {
        type Value = Option<u64>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an optional u64 position")
        }
        fn visit_none<E: DeError>(self) -> Result<Option<u64>, E> {
            Ok(None)
        }
        fn visit_unit<E: DeError>(self) -> Result<Option<u64>, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Option<u64>, D2::Error> {
            deserialize(d).map(Some)
        }
    }
    d.deserialize_option(OV)
}
