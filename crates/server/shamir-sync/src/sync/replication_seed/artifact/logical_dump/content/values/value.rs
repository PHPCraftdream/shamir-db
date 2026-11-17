use std::collections::BTreeSet;
use std::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_bytes::{ByteBuf, Bytes};

/// Explicitly tagged database value. Floats preserve exact bits; integer,
/// decimal, binary, and big-number payloads retain their full logical value.
/// Lists, sets, and map pairs retain insertion order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogicalValue {
    Null,
    Bool(bool),
    Int(i64),
    F64(u64),
    Decimal {
        coefficient: u128,
        scale: u32,
        negative: bool,
    },
    Big(String),
    Str(String),
    Bin(Vec<u8>),
    List(Vec<LogicalValue>),
    Set(Vec<LogicalValue>),
    Map(Vec<(String, LogicalValue)>),
}

impl Serialize for LogicalValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_tuple(2)?;
        match self {
            Self::Null => {
                envelope.serialize_element("null")?;
                envelope.serialize_element(&())?;
            }
            Self::Bool(value) => {
                envelope.serialize_element("bool")?;
                envelope.serialize_element(value)?;
            }
            Self::Int(value) => {
                envelope.serialize_element("int64")?;
                envelope.serialize_element(Bytes::new(&value.to_be_bytes()))?;
            }
            Self::F64(bits) => {
                envelope.serialize_element("f64")?;
                envelope.serialize_element(Bytes::new(&bits.to_be_bytes()))?;
            }
            Self::Decimal {
                coefficient,
                scale,
                negative,
            } if *scale <= 28
                && *coefficient < (1_u128 << 96)
                && (*coefficient != 0 || !*negative) =>
            {
                let mut bytes = [0; 17];
                bytes[0] = u8::from(*negative);
                bytes[1..13].copy_from_slice(&coefficient.to_be_bytes()[4..]);
                bytes[13..].copy_from_slice(&scale.to_be_bytes());
                envelope.serialize_element("dec")?;
                envelope.serialize_element(Bytes::new(&bytes))?;
            }
            Self::Decimal { .. } => {
                return Err(serde::ser::Error::custom(
                    "decimal payload outside supported range",
                ));
            }
            Self::Big(value) if is_canonical_big(value) => {
                envelope.serialize_element("big")?;
                envelope.serialize_element(value)?;
            }
            Self::Big(_) => return Err(serde::ser::Error::custom("noncanonical Big integer")),
            Self::Str(value) => {
                envelope.serialize_element("str")?;
                envelope.serialize_element(value)?;
            }
            Self::Bin(value) => {
                envelope.serialize_element("bin")?;
                envelope.serialize_element(Bytes::new(value))?;
            }
            Self::List(values) => {
                envelope.serialize_element("list")?;
                envelope.serialize_element(values)?;
            }
            Self::Set(values) => {
                if !unique_values(values) {
                    return Err(serde::ser::Error::custom("duplicate value in logical Set"));
                }
                envelope.serialize_element("set")?;
                envelope.serialize_element(values)?;
            }
            Self::Map(entries) => {
                let mut names = BTreeSet::new();
                if entries.iter().any(|(name, _)| !names.insert(name)) {
                    return Err(serde::ser::Error::custom("duplicate logical map key"));
                }
                envelope.serialize_element("map")?;
                envelope.serialize_element(entries)?;
            }
        }
        envelope.end()
    }
}

impl<'de> Deserialize<'de> for LogicalValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_tuple(2, LogicalValueVisitor)
    }
}

struct LogicalValueVisitor;

impl<'de> Visitor<'de> for LogicalValueVisitor {
    type Value = LogicalValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("two-element tagged logical value envelope")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        if sequence.size_hint().is_some_and(|size| size != 2) {
            return Err(de::Error::custom(
                "logical value envelope must contain two elements",
            ));
        }
        let tag: String = next(&mut sequence, "logical value tag")?;
        let value = match tag.as_str() {
            "null" => {
                let _: () = next(&mut sequence, "null payload")?;
                LogicalValue::Null
            }
            "bool" => LogicalValue::Bool(next(&mut sequence, "bool payload")?),
            "int64" => {
                let bytes: ByteBuf = next(&mut sequence, "integer payload")?;
                LogicalValue::Int(i64::from_be_bytes(fixed(bytes)?))
            }
            "f64" => {
                let bytes: ByteBuf = next(&mut sequence, "float payload")?;
                LogicalValue::F64(u64::from_be_bytes(fixed(bytes)?))
            }
            "dec" => {
                let bytes: ByteBuf = next(&mut sequence, "decimal payload")?;
                let bytes: [u8; 17] = fixed(bytes)?;
                if bytes[0] > 1 {
                    return Err(de::Error::custom("invalid decimal sign byte"));
                }
                let mut coefficient_bytes = [0; 16];
                coefficient_bytes[4..].copy_from_slice(&bytes[1..13]);
                let coefficient = u128::from_be_bytes(coefficient_bytes);
                let scale = u32::from_be_bytes(bytes[13..].try_into().map_err(de::Error::custom)?);
                let negative = bytes[0] == 1;
                if scale > 28 || (coefficient == 0 && negative) {
                    return Err(de::Error::custom("noncanonical decimal payload"));
                }
                LogicalValue::Decimal {
                    coefficient,
                    scale,
                    negative,
                }
            }
            "big" => {
                let value: String = next(&mut sequence, "Big integer payload")?;
                if !is_canonical_big(&value) {
                    return Err(de::Error::custom("noncanonical Big integer"));
                }
                LogicalValue::Big(value)
            }
            "str" => LogicalValue::Str(next(&mut sequence, "string payload")?),
            "bin" => {
                LogicalValue::Bin(next::<ByteBuf, _>(&mut sequence, "binary payload")?.into_vec())
            }
            "list" => LogicalValue::List(next(&mut sequence, "list payload")?),
            "set" => {
                let values: Vec<LogicalValue> = next(&mut sequence, "set payload")?;
                if !unique_values(&values) {
                    return Err(de::Error::custom("duplicate value in logical Set"));
                }
                LogicalValue::Set(values)
            }
            "map" => {
                let entries: Vec<(String, LogicalValue)> = next(&mut sequence, "map payload")?;
                let mut names = BTreeSet::new();
                if entries.iter().any(|(name, _)| !names.insert(name)) {
                    return Err(de::Error::custom("duplicate logical map key"));
                }
                LogicalValue::Map(entries)
            }
            _ => return Err(de::Error::custom("unknown logical value tag")),
        };
        if sequence.next_element::<de::IgnoredAny>()?.is_some() {
            return Err(de::Error::custom(
                "logical value envelope has extra elements",
            ));
        }
        Ok(value)
    }
}

fn next<'de, T, A>(sequence: &mut A, field: &'static str) -> Result<T, A::Error>
where
    T: Deserialize<'de>,
    A: SeqAccess<'de>,
{
    sequence
        .next_element()?
        .ok_or_else(|| de::Error::custom(format!("missing {field}")))
}

fn fixed<const N: usize, E: de::Error>(bytes: ByteBuf) -> Result<[u8; N], E> {
    if bytes.len() != N {
        return Err(E::custom(
            "fixed-width logical value payload has wrong length",
        ));
    }
    let mut output = [0; N];
    output.copy_from_slice(&bytes);
    Ok(output)
}

fn is_canonical_big(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    (digits == "0" && !value.starts_with('-')) || (digits != "0" && !digits.starts_with('0'))
}

fn unique_values(values: &[LogicalValue]) -> bool {
    let mut unique = BTreeSet::new();
    values.iter().all(|value| unique.insert(value))
}
