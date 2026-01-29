use fmt::Debug;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use num_bigint::BigInt;
use crate::types::common::{TMap, TSet, new_map_wc};
use std::hash::{Hash, Hasher};
use fxhash::FxHasher;
use std::cmp::Ord;
use serde::de::{self, Deserializer, Visitor, MapAccess, SeqAccess};
use serde::ser::{Serializer, SerializeMap, SerializeSeq};
use std::fmt;
use std::str::FromStr;

pub type UserValue = Value<String>;
pub type InnerValue = Value<u64>;

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
    Key: Deserialize<'de> + Eq + Hash + Ord + Clone + Serialize + Debug,
{
    type Value = Value<Key>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any valid SHAMIR value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> { Ok(Value::Bool(value)) }
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> { Ok(Value::Int(value)) }
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> { Ok(Value::Int(value as i64)) }
    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> { Ok(Value::F64(value)) }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> where E: de::Error { Ok(Value::Str(value.to_owned())) }
    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> { Ok(Value::Str(value)) }
    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E> { Ok(Value::Bin(value.to_vec())) }
    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E> { Ok(Value::Bin(value)) }
    fn visit_none<E>(self) -> Result<Self::Value, E> { Ok(Value::Nil) }
    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error> where D: Deserializer<'de> { deserializer.deserialize_any(self) }
    fn visit_unit<E>(self) -> Result<Self::Value, E> { Ok(Value::Nil) }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where A: SeqAccess<'de>,
    {
        let mut list = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(elem) = seq.next_element()? {
            list.push(elem);
        }
        Ok(Value::List(list))
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where M: MapAccess<'de>,
    {
        let mut inner_map = new_map_wc(map.size_hint().unwrap_or(0));
        while let Some(key_str) = map.next_key::<String>()? {
            let (prefix, real_key) = parse_key_prefix(&key_str);

            let value = match prefix {
                Some("i") => map.next_value::<i64>().map(Value::Int)?,
                Some("u") => map.next_value::<u64>().map(|v| Value::Int(v as i64))?,
                Some("float") => map.next_value::<f64>().map(Value::F64)?,
                Some("dec") => map.next_value::<Decimal>().map(Value::Dec)?,
                Some("big") => {
                    let source: BigIntSource = map.next_value()?;
                    let bigint = match source {
                        BigIntSource::Str(s) => BigInt::from_str(&s).map_err(de::Error::custom)?,
                        BigIntSource::Int(i) => BigInt::from(i),
                        BigIntSource::Uint(u) => BigInt::from(u),
                    };
                    Value::Big(bigint)
                },
                Some("arr") => map.next_value::<Vec<Value<Key>>>().map(Value::List)?,
                Some("set") => map.next_value::<TSet<Value<Key>>>().map(Value::Set)?,
                None => map.next_value()?,
                Some(unknown) => {
                    return Err(de::Error::custom(format!("unknown type prefix: '{}'", unknown)));
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
    Key: Deserialize<'de> + Eq + Hash + Ord + Clone + Serialize + Debug,
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
                if a.is_nan() && b.is_nan() { true } else { a == b }
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
                let mut pairs: Vec<_> = m.iter().collect();
                pairs.sort_by_key(|(k, _)| *k);
                pairs.hash(state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::{new_map, new_set};

    fn calculate_hash<T: Hash>(t: &T) -> u64 {
        let mut hasher = FxHasher::default();
        t.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn test_set_hashing_is_deterministic() {
        let mut set1 = new_set();
        set1.insert(UserValue::Int(1));
        set1.insert(UserValue::Str("hello".to_string()));
        let mut set2 = new_set();
        set2.insert(UserValue::Str("hello".to_string()));
        set2.insert(UserValue::Int(1));
        assert_eq!(set1, set2);
        assert_eq!(calculate_hash(&UserValue::Set(set1)), calculate_hash(&UserValue::Set(set2)));
    }

    #[test]
    fn test_map_hashing_is_deterministic() {
        let mut map1 = new_map();
        map1.insert("a".to_string(), UserValue::Int(1));
        map1.insert("b".to_string(), UserValue::Str("world".to_string()));
        let mut map2 = new_map();
        map2.insert("b".to_string(), UserValue::Str("world".to_string()));
        map2.insert("a".to_string(), UserValue::Int(1));
        assert_eq!(map1, map2);
        assert_eq!(calculate_hash(&UserValue::Map(map1)), calculate_hash(&UserValue::Map(map2)));
    }
}
