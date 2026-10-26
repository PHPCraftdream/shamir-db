use serde::de::{Deserializer, Error, Visitor};
use shamir_types::types::record_id::RecordId;
use std::str::FromStr;

use serde::ser::Serializer;

pub fn serialize<S: Serializer>(id: &RecordId, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(id)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<RecordId, D::Error> {
    deserializer.deserialize_any(RecordIdStrVisitor)
}

pub mod option {
    use super::*;

    pub fn serialize<S: Serializer>(
        id: &Option<RecordId>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match id {
            Some(id) => serializer.serialize_some(&id.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<RecordId>, D::Error> {
        deserializer.deserialize_option(OptionVisitor)
    }

    struct OptionVisitor;

    impl<'de> Visitor<'de> for OptionVisitor {
        type Value = Option<RecordId>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a canonical base58 RecordId string or null")
        }

        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Option<RecordId>, D::Error> {
            super::deserialize(d).map(Some)
        }

        fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }
}

/// STRICT str-only visitor: a `RecordId` on the wire is a canonical base58
/// msgpack str. msgpack bin8/16/32 payloads are rejected via rmp-serde's
/// bin → `visit_bytes` forwarding hitting `invalid_type` here — even when
/// the bin payload's bytes are the ASCII of the canonical token.
struct RecordIdStrVisitor;

impl Visitor<'_> for RecordIdStrVisitor {
    type Value = RecordId;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a canonical base58 RecordId string")
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<RecordId, E> {
        let id = RecordId::from_str(value).map_err(Error::custom)?;
        if id.to_string() != value {
            return Err(Error::custom("non-canonical RecordId"));
        }
        Ok(id)
    }
}
