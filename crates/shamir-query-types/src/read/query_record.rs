//! QueryRecord — read-response row type.

use std::borrow::Cow;
use std::ops::Index;
use std::sync::OnceLock;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use shamir_types::types::value::QueryValue;

use crate::write::InsertedRecord;

/// Read response row.
///
/// `Direct` is the hot path (no `serde_json::Map` allocation per record);
/// `Json` is the legacy shape for paths not yet migrated.  Both variants
/// serialize to byte-identical output on the wire.
#[derive(Debug)]
pub enum QueryRecord {
    /// Projected row coming from the engine read path — zero extra
    /// allocation beyond the `QueryValue` produced by `project_value`.
    Direct(QueryValue),
    /// Legacy deserialized row (from the wire / test fixtures).
    Json(serde_json::Value),
    /// A write-result row carried straight through from a DML op.
    ///
    /// Wraps the [`InsertedRecord`] that `execute_*` already built, so the
    /// batch layer can fold a `WriteResult` into a `QueryResult` WITHOUT
    /// re-materialising each row into a `serde_json::Value` (the old
    /// `write_result_to_query_result` double-build). Serialization delegates
    /// to [`InsertedRecord`]'s impl, so the wire bytes are byte-identical to
    /// the former `Json(serde_json::to_value(rec))` path.
    ///
    /// The second field is a lazy json cache: the wire/serialize path never
    /// touches it (it delegates straight to `InsertedRecord`, the C1 win),
    /// while the in-process keyed accessors (`get` / `Index<&str>` /
    /// `as_json`) materialise the row's `serde_json::Value` ONCE on first use
    /// and lend a reference into it — so `records[i]["field"]` keeps working
    /// exactly as it did when inserts produced `Json`. `OnceLock::new()` is
    /// alloc-free, so the cache is zero-cost on the hot path that never reads
    /// a field.
    Inserted(InsertedRecord, OnceLock<serde_json::Value>),
}

impl Clone for QueryRecord {
    fn clone(&self) -> Self {
        match self {
            QueryRecord::Direct(v) => QueryRecord::Direct(v.clone()),
            QueryRecord::Json(v) => QueryRecord::Json(v.clone()),
            // The lazy cache is per-instance; a clone starts cold (it
            // re-materialises on first access). `OnceLock` is not `Clone`,
            // which is why this impl is manual.
            QueryRecord::Inserted(rec, _) => QueryRecord::Inserted(rec.clone(), OnceLock::new()),
        }
    }
}

// ── serde ────────────────────────────────────────────────────────────────────

impl Serialize for QueryRecord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            QueryRecord::Direct(v) => v.serialize(s),
            QueryRecord::Json(v) => v.serialize(s),
            QueryRecord::Inserted(rec, _) => rec.serialize(s),
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

impl From<InsertedRecord> for QueryRecord {
    fn from(r: InsertedRecord) -> Self {
        QueryRecord::Inserted(r, OnceLock::new())
    }
}

impl From<QueryRecord> for serde_json::Value {
    fn from(r: QueryRecord) -> Self {
        match r {
            QueryRecord::Json(v) => v,
            QueryRecord::Direct(qv) => qv.into(),
            QueryRecord::Inserted(rec, _) => rec.as_json().into_owned(),
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
            // Materialise ONCE into the lazy cache (slow path — accessors /
            // tests only; the wire path serialises via the byte-identical
            // Serialize impl and never reaches here). Subsequent accessors
            // borrow the cached value.
            QueryRecord::Inserted(rec, cache) => {
                Cow::Borrowed(cache.get_or_init(|| rec.as_json().into_owned()))
            }
        }
    }

    /// Look up a field by name, returning a borrowed reference.
    ///
    /// Works for `Json` and `Inserted` (the latter materialises its lazy json
    /// cache once and borrows into it).  For the `Direct` variant this returns
    /// `None` — use `as_json().get(key)` with an explicit local binding when
    /// the record may be `Direct`.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        match self {
            QueryRecord::Json(v) => v.get(key),
            QueryRecord::Inserted(rec, cache) => {
                cache.get_or_init(|| rec.as_json().into_owned()).get(key)
            }
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
            // Inserted materialises its lazy json cache once and lends a
            // reference into it, so `records[i]["field"]` works exactly as it
            // did when inserts produced `Json`.
            QueryRecord::Inserted(rec, cache) => cache
                .get_or_init(|| rec.as_json().into_owned())
                .get(key)
                .unwrap_or(&NULL),
            // Direct has no cache (it is the read hot path, kept alloc-free)
            // and cannot lend a reference into a materialised value. This
            // branch panics to surface migration sites; production read paths
            // use `.as_json()` or the wire-deserialised `Json` form.
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
    use shamir_types::types::common::new_map_wc;
    use shamir_types::types::record_id::RecordId;
    use shamir_types::types::value::QueryValue;

    use super::QueryRecord;
    use crate::write::InsertedRecord;

    /// Verify that `Direct` and `Json` variants produce byte-identical
    /// `serde_json` serialization output for the same logical value.
    #[test]
    fn byte_identical_to_json_value() {
        // Build a QueryValue map with the same content as the json! literal.
        let mut map = new_map_wc(3);
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

    /// C1 byte-identity: `QueryRecord::Inserted(rec)` must serialise
    /// byte-for-byte identically to the OLD `write_result_to_query_result`
    /// path — `QueryRecord::Json(serde_json::to_value(rec))` — on BOTH the
    /// JSON and msgpack wire encodings, including the `_id` injection that
    /// `InsertedRecord::serialize` performs.
    #[test]
    fn inserted_variant_byte_identical_to_old_json_path() {
        let mut map = new_map_wc(3);
        map.insert("name".to_string(), QueryValue::Str("widget".to_string()));
        map.insert("qty".to_string(), QueryValue::Int(42));
        map.insert("score".to_string(), QueryValue::F64(3.5));
        let id = RecordId::system("test-id-00");
        let rec = InsertedRecord::Direct {
            id,
            fields: QueryValue::Map(map),
        };

        // NEW path: carry the InsertedRecord through unchanged.
        let new_rec = QueryRecord::Inserted(rec.clone(), std::sync::OnceLock::new());
        // OLD path: re-materialise via serde_json::to_value, wrap as Json.
        let old_rec = QueryRecord::Json(serde_json::to_value(&rec).unwrap());

        // JSON wire bytes identical.
        let new_json = serde_json::to_vec(&new_rec).unwrap();
        let old_json = serde_json::to_vec(&old_rec).unwrap();
        assert_eq!(
            new_json, old_json,
            "Inserted vs old Json path must emit identical JSON bytes"
        );

        // msgpack wire bytes identical (named maps — the real transport shape).
        let new_mp = rmp_serde::to_vec_named(&new_rec).unwrap();
        let old_mp = rmp_serde::to_vec_named(&old_rec).unwrap();
        assert_eq!(
            new_mp, old_mp,
            "Inserted vs old Json path must emit identical msgpack bytes"
        );
    }

    /// Regression (C1 fix): in-process keyed access — `record["field"]`
    /// (`Index<&str>`) and `record.get("field")` — must work on the
    /// `Inserted` variant exactly as it did when inserts produced `Json`.
    /// Before the lazy-cache fix, `Inserted` panicked on `Index<&str>` and
    /// returned `None` from `get`, breaking `resp.results[..].records[i]["x"]`
    /// for any in-process consumer (shamir-db e2e tests caught it).
    #[test]
    fn inserted_variant_keyed_access_works() {
        let mut map = new_map_wc(2);
        map.insert("name".to_string(), QueryValue::Str("widget".to_string()));
        map.insert("qty".to_string(), QueryValue::Int(42));
        let rec = InsertedRecord::Direct {
            id: RecordId::system("test-id-01"),
            fields: QueryValue::Map(map),
        };
        let qr: QueryRecord = rec.into();

        // Index<&str> — the operator the failing tests used.
        assert_eq!(qr["name"], serde_json::json!("widget"));
        assert_eq!(qr["qty"], serde_json::json!(42));
        // get — borrowed reference into the lazily-materialised cache.
        assert_eq!(qr.get("name"), Some(&serde_json::json!("widget")));
        assert_eq!(qr.get("qty"), Some(&serde_json::json!(42)));
        // Missing key → Null via Index, None via get (matches Json behaviour).
        assert_eq!(qr["nope"], serde_json::Value::Null);
        assert_eq!(qr.get("nope"), None);
    }
}
