//! Hash buckets for distinct aggregates. Equal-under-compare values hash alike
//! recursively, including unordered sets and maps; collisions are compared.

use crate::compare;
use crate::functions::numeric_exact::ExactNumber;
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
        QueryValue::Int(_) | QueryValue::F64(_) | QueryValue::Dec(_) | QueryValue::Big(_) => {
            state.write_u8(2);
            ExactNumber::from_value(v).hash(state);
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

pub(super) fn same_bucket(a: &QueryValue, b: &QueryValue) -> bool {
    compare::compare(a, b) == Ordering::Equal
}
