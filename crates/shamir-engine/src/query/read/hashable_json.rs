//! `HashableJson` — `Hash + Eq` wrapper for `serde_json::Value`.
//!
//! Used by `apply_distinct` to de-duplicate JSON records without
//! serialising each one to a `String`.

use serde_json as json;

/// Wrapper that gives `json::Value` a `Hash + Eq` implementation backed by
/// a structural walk of the tree. `json::Value::eq` is structural already;
/// the missing piece was `Hash`, which the standard library can't provide
/// because `serde_json::Number` carries non-totally-ordered floats. We
/// hash float bits — the same canonical form the old `to_string()` path
/// produced, just without allocating a `String` per record.
pub(super) struct HashableJson(pub(super) json::Value);

impl PartialEq for HashableJson {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for HashableJson {}

impl std::hash::Hash for HashableJson {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        hash_json(&self.0, h);
    }
}

pub(super) fn hash_json<H: std::hash::Hasher>(v: &json::Value, h: &mut H) {
    use std::hash::Hash;
    match v {
        json::Value::Null => h.write_u8(0),
        json::Value::Bool(b) => {
            h.write_u8(1);
            h.write_u8(*b as u8);
        }
        json::Value::Number(n) => {
            h.write_u8(2);
            if let Some(i) = n.as_i64() {
                h.write_u8(0);
                h.write_i64(i);
            } else if let Some(u) = n.as_u64() {
                h.write_u8(1);
                h.write_u64(u);
            } else if let Some(f) = n.as_f64() {
                h.write_u8(2);
                h.write_u64(f.to_bits());
            } else {
                h.write_u8(3);
                // Falls back through Display; rare path.
                n.to_string().hash(h);
            }
        }
        json::Value::String(s) => {
            h.write_u8(3);
            h.write(s.as_bytes());
            h.write_u8(0xff);
        }
        json::Value::Array(arr) => {
            h.write_u8(4);
            h.write_u64(arr.len() as u64);
            for x in arr {
                hash_json(x, h);
            }
        }
        json::Value::Object(map) => {
            h.write_u8(5);
            h.write_u64(map.len() as u64);
            for (k, v) in map {
                h.write(k.as_bytes());
                h.write_u8(0);
                hash_json(v, h);
            }
        }
    }
}
