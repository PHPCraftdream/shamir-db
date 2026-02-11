#![allow(deprecated)]

use crate::core::interner::InternedKey;
use crate::types::common::{new_map_wc, TMap, TSet};
use bytes::Bytes;
use fxhash::FxHasher;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::any::TypeId;
use std::cmp::Ord;
use std::fmt::{self, Debug};
use std::hash::{Hash, Hasher};
use std::str::FromStr;

/// User-facing value type with string keys
///
/// **DEPRECATED & FOR TESTS ONLY**
///
/// This type should only be used in tests for convenience.
/// Production code should use `InnerValue` directly with interning.
#[deprecated(
    since = "0.1.0",
    note = "Use InnerValue instead. UserValue is for tests only."
)]
pub type UserValue = Value<String>;
pub type InnerValue = Value<InternedKey>;

#[derive(Debug, Clone)]
pub enum Value<Key: Eq + Hash + Ord + Clone + Serialize + Debug> {
    Nil,
    Bool(bool),
    Int(i64),
    F64(f64),
    Dec(Decimal),
    Big(BigInt),
    Str(String),
    Bin(Vec<u8>),
    List(Vec<Value<Key>>),
    Set(TSet<Value<Key>>),
    Map(TMap<Key, Value<Key>>),
}

impl<Key: Eq + Hash + Ord + Clone + Serialize + for<'de> Deserialize<'de> + Debug + 'static>
    Value<Key>
{
    /// Serializes the `Value` into `Bytes` using MessagePack.
    pub fn to_bytes(&self) -> Bytes {
        Bytes::from(rmp_serde::to_vec(self).expect("Failed to serialize Value"))
    }

    /// Deserializes bytes into a `Value` using MessagePack.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self, rmp_serde::decode::Error> {
        rmp_serde::from_slice(bytes.as_ref())
    }
}

impl<Key: Eq + Hash + Ord + Clone + Serialize + Debug> Serialize for Value<Key> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Value::Nil => serializer.serialize_unit(),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Int(i) => serializer.serialize_i64(*i),
            Value::F64(f) => serializer.serialize_f64(*f),
            Value::Dec(d) => serializer.serialize_str(&d.to_string()),
            Value::Big(b) => serializer.serialize_str(&b.to_string()),
            Value::Str(s) => serializer.serialize_str(s),
            Value::Bin(b) => serializer.serialize_bytes(b),
            Value::List(l) => {
                let mut seq = serializer.serialize_seq(Some(l.len()))?;
                for element in l {
                    seq.serialize_element(element)?;
                }
                seq.end()
            }
            Value::Set(s) => {
                let mut seq = serializer.serialize_seq(Some(s.len()))?;
                for element in s {
                    seq.serialize_element(element)?;
                }
                seq.end()
            }
            Value::Map(m) => {
                let mut map = serializer.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

fn parse_key_prefix(key: &str) -> (Option<&str>, &str) {
    if let Some((prefix, rest)) = key.split_once(':') {
        (Some(prefix), rest)
    } else {
        (None, key)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BigIntSource {
    Str(String),
    Int(i64),
    Uint(u64),
}

struct ValueVisitor<K>(std::marker::PhantomData<K>);

impl<'de, Key> Visitor<'de> for ValueVisitor<Key>
where
    Key: Deserialize<'de> + Eq + Hash + Ord + Clone + Serialize + Debug + 'static,
{
    type Value = Value<Key>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any valid SHAMIR value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Int(value))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Int(value as i64))
    }
    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(Value::F64(value))
    }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Str(value.to_owned()))
    }
    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::Str(value))
    }
    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E> {
        Ok(Value::Bin(value.to_vec()))
    }
    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        Ok(Value::Bin(value))
    }
    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Nil)
    }
    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Nil)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut list = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(elem) = seq.next_element()? {
            list.push(elem);
        }
        Ok(Value::List(list))
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        // For InnerValue (Key=u64) or other non-string keys, use direct deserialization.
        if TypeId::of::<Key>() != TypeId::of::<String>() {
            let mut inner_map = new_map_wc(map.size_hint().unwrap_or(0));
            while let Some((key, value)) = map.next_entry()? {
                inner_map.insert(key, value);
            }
            return Ok(Value::Map(inner_map));
        }

        // For UserValue (Key=String), use the special prefix-parsing logic.
        let mut inner_map = new_map_wc(map.size_hint().unwrap_or(0));
        while let Some(key_str) = map.next_key::<String>()? {
            let (prefix, real_key) = parse_key_prefix(&key_str);

            let value = match prefix {
                Some("i") => map.next_value::<i64>().map(Value::Int)?,
                Some("u") => map.next_value::<u64>().map(|v| Value::Int(v as i64))?,
                Some("float") => map.next_value::<f64>().map(Value::F64)?,
                Some("dec") => {
                    let s: String = map.next_value()?;
                    // Validate that it's a valid decimal, but store as string
                    let _ = Decimal::from_str(&s).map_err(de::Error::custom)?;
                    Value::Str(s)
                }
                Some("big") => {
                    let source: BigIntSource = map.next_value()?;
                    // Validate that it's a valid bigint, but store as string
                    let s = match source {
                        BigIntSource::Str(s) => {
                            let _ = BigInt::from_str(&s).map_err(de::Error::custom)?;
                            s
                        }
                        BigIntSource::Int(i) => BigInt::from(i).to_string(),
                        BigIntSource::Uint(u) => BigInt::from(u).to_string(),
                    };
                    Value::Str(s)
                }
                Some("arr") => map.next_value::<Vec<Value<Key>>>().map(Value::List)?,
                Some("set") => map.next_value::<TSet<Value<Key>>>().map(Value::Set)?,
                None => map.next_value()?,
                Some(unknown) => {
                    return Err(de::Error::custom(format!(
                        "unknown type prefix: '{}'",
                        unknown
                    )));
                }
            };

            let map_key = Key::deserialize(de::IntoDeserializer::into_deserializer(real_key))?;
            inner_map.insert(map_key, value);
        }
        Ok(Value::Map(inner_map))
    }
}

impl<'de, Key> Deserialize<'de> for Value<Key>
where
    Key: Deserialize<'de> + Eq + Hash + Ord + Clone + Serialize + Debug + 'static,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ValueVisitor(std::marker::PhantomData))
    }
}

impl<Key: Eq + Hash + Ord + Clone + Serialize + Debug> PartialEq for Value<Key> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::F64(a), Value::F64(b)) => {
                if a.is_nan() && b.is_nan() {
                    true
                } else {
                    a == b
                }
            }
            (Value::Dec(a), Value::Dec(b)) => a == b,
            (Value::Big(a), Value::Big(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bin(a), Value::Bin(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Set(a), Value::Set(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            _ => false,
        }
    }
}

impl<Key: Eq + Hash + Ord + Clone + Serialize + Debug> Eq for Value<Key> {}

impl<Key: Eq + Hash + Ord + Clone + Serialize + Debug> Hash for Value<Key> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Value::Nil => {}
            Value::Bool(b) => b.hash(state),
            Value::Int(i) => i.hash(state),
            Value::F64(f) => f.to_bits().hash(state),
            Value::Dec(d) => d.hash(state),
            Value::Big(b) => b.hash(state),
            Value::Str(s) => s.hash(state),
            Value::Bin(b) => b.hash(state),
            Value::List(l) => l.hash(state),
            Value::Set(s) => {
                let mut xor_sum: u64 = 0;
                for v in s {
                    let mut hasher = FxHasher::default();
                    v.hash(&mut hasher);
                    xor_sum ^= hasher.finish();
                }
                xor_sum.hash(state);
            }
            Value::Map(m) => {
                // XOR approach - order-independent and no allocation
                let mut xor_sum: u64 = 0;
                for (k, v) in m {
                    let mut hasher = FxHasher::default();
                    k.hash(&mut hasher);
                    v.hash(&mut hasher);
                    xor_sum ^= hasher.finish();
                }
                xor_sum.hash(state);
            }
        }
    }
}
