use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use crate::types::common::{TMap, TSet};
use std::hash::{Hash, Hasher};

// Removed PartialEq from derive to implement it manually.
// Added Serialize, Deserialize as per project constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    Nil,
    Bool(bool),

    Int(i64),
    F64(f64),
    Dec(Decimal),

    Str(String),
    Bin(Vec<u8>),

    List(Vec<Value>),
    Set(TSet<Value>),
    Map(TMap<String, Value>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::F64(a), Value::F64(b)) => {
                // Consider two NaNs to be equal for our purposes.
                if a.is_nan() && b.is_nan() {
                    true
                } else {
                    a == b
                }
            }
            (Value::Dec(a), Value::Dec(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bin(a), Value::Bin(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Set(a), Value::Set(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            _ => false,
        }
    }
}

// Now that we have a custom PartialEq, we can implement Eq.
impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Value::Nil => {}
            Value::Bool(b) => b.hash(state),
            Value::Int(i) => i.hash(state),
            Value::F64(f) => f.to_bits().hash(state),
            Value::Dec(d) => d.hash(state),
            Value::Str(s) => s.hash(state),
            Value::Bin(b) => b.hash(state),
            Value::List(l) => l.hash(state),
            Value::Set(s) => {
                // To create a stable hash for a HashSet, we can't rely on iteration order.
                // A common method is to XOR the hashes of all elements.
                let mut xor_sum: u64 = 0;
                for v in s {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    v.hash(&mut hasher);
                    xor_sum ^= hasher.finish();
                }
                xor_sum.hash(state);
            }
            Value::Map(m) => {
                // For a HashMap, we can sort by keys to get a deterministic hash.
                let mut pairs: Vec<_> = m.iter().collect();
                pairs.sort_by_key(|(k, _)| *k);
                pairs.hash(state);
            }
        }
    }
}
