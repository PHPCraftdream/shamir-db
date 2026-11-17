use std::collections::BTreeSet;
use std::fmt;
use std::io::Cursor;

use serde::de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::canonical_lengths::validate_canonical_lengths;
use crate::replication_seed::codec::FRAME_LIMIT;
use crate::replication_seed::encode_bounded::encode_bounded;
use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

const DEPTH_LIMIT: usize = 64;
const CONTAINER_LIMIT: usize = 65_536;
const WORKING_LIMIT: usize = 64 * 1024 * 1024;

/// Encode named MessagePack to a single canonical representation. Format maps
/// are sorted by their UTF-8 field bytes; arrays and tagged logical values
/// preserve sequence order. Seed wire encoding continues to use its existing
/// permissive helpers unchanged.
pub(crate) fn encode_canonical_named<T: Serialize>(
    value: &T,
    limit: usize,
) -> Result<Vec<u8>, SeedError> {
    let limit = limit.min(FRAME_LIMIT);
    let encoded = encode_bounded(value, limit)?;
    validate_canonical_lengths(&encoded, false)?;
    let mut node = decode_node(&encoded)?;
    sort_maps(&mut node);
    let canonical = rmp_serde::to_vec_named(&node).map_err(|error| invalid(error.to_string()))?;
    if canonical.len() > limit {
        return Err(invalid("canonical MessagePack byte budget exceeded"));
    }
    validate_canonical_lengths(&canonical, true)?;
    Ok(canonical)
}

/// Decode strict canonical named MessagePack. Typed values must serialize back
/// to the same canonical wire representation; this rejects byte-sequence
/// substitutes accepted by serde_bytes and types that silently normalize data.
pub(crate) fn decode_canonical_named<T: DeserializeOwned + Serialize>(
    bytes: &[u8],
) -> Result<T, SeedError> {
    if bytes.len() > FRAME_LIMIT || bytes.len().saturating_mul(4) > WORKING_LIMIT {
        return Err(invalid("canonical MessagePack working budget exceeded"));
    }
    validate_canonical_lengths(bytes, true)?;

    let mut canonical_node = decode_node(bytes)?;
    sort_maps(&mut canonical_node);
    let canonical =
        rmp_serde::to_vec_named(&canonical_node).map_err(|error| invalid(error.to_string()))?;
    if canonical.as_slice() != bytes {
        return Err(invalid("noncanonical named MessagePack"));
    }
    drop(canonical_node);
    drop(canonical);

    let mut decoder = rmp_serde::Deserializer::new(Cursor::new(bytes));
    decoder.set_max_depth(DEPTH_LIMIT);
    let value = T::deserialize(&mut decoder).map_err(|error| invalid(error.to_string()))?;
    if decoder.position()
        != u64::try_from(bytes.len()).map_err(|error| invalid(error.to_string()))?
    {
        return Err(invalid("trailing MessagePack bytes"));
    }
    let typed_bytes =
        rmp_serde::to_vec_named(&value).map_err(|error| invalid(error.to_string()))?;
    if typed_bytes.len() > FRAME_LIMIT {
        return Err(invalid("canonical MessagePack byte budget exceeded"));
    }
    let mut typed_node = decode_node(&typed_bytes)?;
    sort_maps(&mut typed_node);
    let typed_canonical =
        rmp_serde::to_vec_named(&typed_node).map_err(|error| invalid(error.to_string()))?;
    if typed_canonical.as_slice() != bytes {
        return Err(invalid(
            "typed MessagePack value is not canonically encoded",
        ));
    }
    Ok(value)
}

fn decode_node(bytes: &[u8]) -> Result<Node, SeedError> {
    let mut decoder = rmp_serde::Deserializer::new(Cursor::new(bytes));
    decoder.set_max_depth(DEPTH_LIMIT);
    let node = Node::deserialize(&mut decoder).map_err(|error| invalid(error.to_string()))?;
    if decoder.position()
        != u64::try_from(bytes.len()).map_err(|error| invalid(error.to_string()))?
    {
        return Err(invalid("trailing MessagePack bytes"));
    }
    Ok(node)
}

#[derive(Debug)]
enum Node {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float32(u32),
    Float64(u64),
    String(String),
    Binary(Vec<u8>),
    Array(Vec<Node>),
    Map(Vec<(String, Node)>),
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Signed(value) => serializer.serialize_i64(*value),
            Self::Unsigned(value) => serializer.serialize_u64(*value),
            Self::Float32(bits) => serializer.serialize_f32(f32::from_bits(*bits)),
            Self::Float64(bits) => serializer.serialize_f64(f64::from_bits(*bits)),
            Self::String(value) => serializer.serialize_str(value),
            Self::Binary(value) => serializer.serialize_bytes(value),
            Self::Array(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(value)?;
                }
                sequence.end()
            }
            Self::Map(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded MessagePack value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Node, E> {
        Ok(Node::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Node, E> {
        Ok(Node::Signed(value))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Node, E> {
        Ok(Node::Unsigned(value))
    }
    fn visit_f32<E: de::Error>(self, value: f32) -> Result<Node, E> {
        Ok(Node::Float32(value.to_bits()))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Node, E> {
        Ok(Node::Float64(value.to_bits()))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Node, E> {
        Ok(Node::String(value.to_owned()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Node, E> {
        Ok(Node::String(value))
    }
    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Node, E> {
        Ok(Node::Binary(value.to_vec()))
    }
    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Node, E> {
        Ok(Node::Binary(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Node, A::Error> {
        let capacity = sequence.size_hint().unwrap_or(0).min(CONTAINER_LIMIT);
        let mut values = Vec::with_capacity(capacity);
        while let Some(value) = sequence.next_element()? {
            values.push(value);
        }
        Ok(Node::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let capacity = map.size_hint().unwrap_or(0).min(CONTAINER_LIMIT);
        let mut entries = Vec::with_capacity(capacity);
        let mut names = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !names.insert(key.clone()) {
                return Err(de::Error::custom("duplicate named MessagePack field"));
            }
            let value = map.next_value()?;
            entries.push((key, value));
        }
        Ok(Node::Map(entries))
    }
}

fn sort_maps(node: &mut Node) {
    match node {
        Node::Array(values) => values.iter_mut().for_each(sort_maps),
        Node::Map(entries) => {
            entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
            entries.iter_mut().for_each(|(_, value)| sort_maps(value));
        }
        Node::Null
        | Node::Bool(_)
        | Node::Signed(_)
        | Node::Unsigned(_)
        | Node::Float32(_)
        | Node::Float64(_)
        | Node::String(_)
        | Node::Binary(_) => {}
    }
}
