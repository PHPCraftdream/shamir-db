//! Hash buckets for distinct aggregates. The comparator's mixed-numeric
//! equality may be non-transitive, so it must never be an Eq implementation
//! on a set/map key. It is used only inside the matching hash bucket, retaining
//! legacy first-representative semantics.
//!
//! Equal-under-compare values hash alike recursively (including unordered
//! Set/Map). Expected O(N) for well-distributed keys, not a worst-case bound:
//! collisions and large values with the same lossy f64 image can make bucket
//! scans linear.

use crate::compare;
use num_bigint::Sign;
use rust_decimal::prelude::ToPrimitive;
use shamir_collections::THasher;
use shamir_types::types::value::QueryValue;
use std::cmp::Ordering;
use std::hash::{BuildHasher, Hash};

pub(super) fn hash_value(v: &QueryValue) -> u64 {
    THasher::default().hash_one(HashView(v))
}

struct HashView<'a>(&'a QueryValue);

impl std::hash::Hash for HashView<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        hash_into(self.0, state);
    }
}

fn hash_into<H: std::hash::Hasher>(v: &QueryValue, state: &mut H) {
    match v {
        QueryValue::Null => state.write_u8(0),
        QueryValue::Bool(b) => {
            state.write_u8(1);
            b.hash(state);
        }
        QueryValue::Int(n) => {
            state.write_u8(2);
            hash_f64(*n as f64, state);
        }
        QueryValue::F64(f) => {
            state.write_u8(2);
            hash_f64(*f, state);
        }
        QueryValue::Dec(d) => {
            state.write_u8(2);
            hash_f64(d.to_f64().unwrap_or(f64::NAN), state);
        }
        QueryValue::Big(b) => {
            state.write_u8(2);
            hash_f64(
                b.to_f64().unwrap_or_else(|| {
                    if b.sign() == Sign::Minus {
                        f64::NEG_INFINITY
                    } else {
                        f64::INFINITY
                    }
                }),
                state,
            );
        }
        QueryValue::Str(s) => {
            state.write_u8(3);
            s.hash(state);
        }
        QueryValue::Bin(b) => {
            state.write_u8(4);
            b.hash(state);
        }
        QueryValue::List(xs) => {
            state.write_u8(5);
            xs.len().hash(state);
            for x in xs {
                hash_into(x, state);
            }
        }
        QueryValue::Set(xs) => {
            state.write_u8(6);
            let mut hashes: Vec<_> = xs.iter().map(hash_one).collect();
            hashes.sort_unstable();
            hashes.hash(state);
        }
        QueryValue::Map(xs) => {
            state.write_u8(7);
            let mut entries: Vec<_> = xs.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            for (key, value) in entries {
                key.hash(state);
                hash_into(value, state);
            }
        }
    }
}

fn hash_one(v: &QueryValue) -> u64 {
    THasher::default().hash_one(HashView(v))
}

fn hash_f64<H: std::hash::Hasher>(value: f64, state: &mut H) {
    let bits = if value.is_nan() {
        f64::NAN.to_bits()
    } else if value == 0.0 {
        0.0f64.to_bits()
    } else {
        value.to_bits()
    };
    bits.hash(state);
}

pub(super) fn same_bucket(a: &QueryValue, b: &QueryValue) -> bool {
    compare::compare(a, b) == Ordering::Equal
}
