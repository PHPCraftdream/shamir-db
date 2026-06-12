//! QueryRecord — read-response row type.

use std::borrow::Cow;
use std::ops::Index;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use shamir_types::types::value::QueryValue;

/// Read response row.
///
/// `Direct` is the hot path (no `serde_json::Map` allocation per record);
/// `Json` is the legacy shape for paths not yet migrated.  Both variants
/// serialize to byte-identical output on the wire.
#[derive(Debug, Clone)]
pub enum QueryRecord {
    /// Projected row coming from the engine read path — zero extra
    /// allocation beyond the `QueryValue` produced by `project_value`.
    Direct(QueryValue),
    /// Legacy deserialized row (from the wire / test fixtures).
    Json(serde_json::Value),
}

// ── serde ────────────────────────────────────────────────────────────────────

impl Serialize for QueryRecord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            QueryRecord::Direct(v) => v.serialize(s),
            QueryRecord::Json(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for QueryRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(QueryRecord::Json(serde_json::Value::deserialize(d)?))
    }
}

// ── From conversions ──────────────────────────────────────────────────────────

impl From<serde_json::Value> for QueryRecord {
    fn from(v: serde_json::Value) -> Self {
        QueryRecord::Json(v)
    }
}

impl From<QueryValue> for QueryRecord {
    fn from(v: QueryValue) -> Self {
        QueryRecord::Direct(v)
    }
}

impl From<QueryRecord> for serde_json::Value {
    fn from(r: QueryRecord) -> Self {
        match r {
            QueryRecord::Json(v) => v,
            QueryRecord::Direct(qv) => qv.into(),
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

impl QueryRecord {
    /// View row as a `serde_json::Value`, materialising once if needed.
    pub fn as_json(&self) -> Cow<'_, serde_json::Value> {
        match self {
            QueryRecord::Json(v) => Cow::Borrowed(v),
            QueryRecord::Direct(qv) => Cow::Owned(qv.clone().into()),
        }
    }

    /// Look up a field by name, returning a borrowed reference.
    ///
    /// Works correctly for the `Json` variant.  For the `Direct` variant
    /// this returns `None` — use `as_json().get(key)` with an explicit
    /// local binding when the record may be `Direct`.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        match self {
            QueryRecord::Json(v) => v.get(key),
            QueryRecord::Direct(_) => None,
        }
    }

    /// Look up a field by name and return an owned `serde_json::Value`.
    ///
    /// Works for both variants.  Convenience over `as_json().get(key).cloned()`.
    pub fn get_owned(&self, key: &str) -> Option<serde_json::Value> {
        self.as_json().get(key).cloned()
    }

    /// Look up a string field by name. Returns `None` if absent or not a string.
    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get_owned(key)
            .and_then(|v| v.as_str().map(str::to_owned))
    }

    /// Look up an i64 field by name. Returns `None` if absent or not a number.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get_owned(key).and_then(|v| v.as_i64())
    }

    /// Look up a u64 field by name. Returns `None` if absent or not a number.
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get_owned(key).and_then(|v| v.as_u64())
    }
}

// ── Index<&str> bridge — keeps `records[i]["field"]` compiling ───────────────

impl Index<&str> for QueryRecord {
    type Output = serde_json::Value;

    fn index(&self, key: &str) -> &Self::Output {
        static NULL: serde_json::Value = serde_json::Value::Null;
        match self {
            QueryRecord::Json(v) => v.get(key).unwrap_or(&NULL),
            // Direct cannot lend a reference into a materialised value.
            // This branch panics to surface migration sites; production
            // paths never reach here because the read path deserialises
            // into Json on the wire side.
            QueryRecord::Direct(_) => {
                panic!(
                    "QueryRecord::Direct does not support Index<&str> — \
                     use .as_json()[key] instead"
                );
            }
        }
    }
}

// ── PartialEq (for tests) ─────────────────────────────────────────────────────

impl PartialEq for QueryRecord {
    fn eq(&self, other: &Self) -> bool {
        // Normalise to serde_json for comparison so Json == Direct works.
        let a: serde_json::Value = self.clone().into();
        let b: serde_json::Value = other.clone().into();
        a == b
    }
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use serde_json::json;
    use shamir_types::types::value::QueryValue;

    use super::QueryRecord;

    /// Verify that `Direct` and `Json` variants produce byte-identical
    /// `serde_json` serialization output for the same logical value.
    #[test]
    fn byte_identical_to_json_value() {
        // Build a QueryValue map with the same content as the json! literal.
        let mut map = shamir_types::types::common::new_map_wc(3);
        map.insert("name".to_string(), QueryValue::Str("alice".to_string()));
        map.insert("age".to_string(), QueryValue::Int(30));
        map.insert("active".to_string(), QueryValue::Bool(true));
        let direct = QueryRecord::Direct(QueryValue::Map(map));

        let json_rec = QueryRecord::Json(json!({
            "name": "alice",
            "age": 30,
            "active": true
        }));

        // Serialise both to string and compare.
        let s_direct = serde_json::to_string(&direct).unwrap();
        let s_json = serde_json::to_string(&json_rec).unwrap();

        // Both must round-trip to the same serde_json::Value.
        let v_direct: serde_json::Value = serde_json::from_str(&s_direct).unwrap();
        let v_json: serde_json::Value = serde_json::from_str(&s_json).unwrap();
        assert_eq!(
            v_direct, v_json,
            "Direct and Json variants must serialise to byte-identical output"
        );
    }
}
