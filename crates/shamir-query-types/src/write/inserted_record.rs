//! Allocation-free write-result record for INSERT/UPSERT hot paths.

use std::borrow::Cow;

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{QueryValue, Value};

/// A single record returned inside [`WriteResult`](super::WriteResult).
///
/// Wire shape is a JSON/msgpack map of fields plus `_id` — emitted
/// byte-identically by both variants.
///
/// * `Direct` — built from `QueryValue` + `RecordId` without allocating
///   a `serde_json::Map`. Hot path: insert/upsert result construction.
/// * `Json` — wraps a `serde_json::Value` for call sites that already
///   hold one (update, upsert, admin ops). Backward-compatible.
#[derive(Debug, Clone)]
pub enum InsertedRecord {
    /// Zero-copy insert result: serialize directly from [`QueryValue`] +
    /// optional [`RecordId`] without an intermediate `serde_json::Map`
    /// allocation.
    ///
    /// When `id` is `Some`, `_id` is injected in sorted-key position
    /// (matching what `serde_json::Map` would emit). When `None`, only
    /// the `fields` map is serialized — used by UPDATE-RETURNING and
    /// SET-UPDATE where the wire result carries no `_id`.
    Direct {
        id: Option<RecordId>,
        fields: QueryValue,
    },
    /// Legacy: a fully-built `serde_json::Value` map. Kept for call
    /// sites (update, upsert) that already have one.
    Json(serde_json::Value),
}

// ── Serialize ──────────────────────────────────────────────────────────────

impl Serialize for InsertedRecord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            InsertedRecord::Json(v) => v.serialize(serializer),
            InsertedRecord::Direct { id, fields } => {
                // Emit in the same sorted-key order as serde_json::Map
                // (which uses a BTreeMap-ordered structure in this build),
                // so msgpack bytes are byte-identical to the old Json path.
                let id_str = id.as_ref().map(|r| r.to_string());
                match fields {
                    Value::Map(m) => {
                        let extra = if id_str.is_some() { 1 } else { 0 };
                        let field_count = m.len() + extra;
                        let mut map = serializer.serialize_map(Some(field_count))?;
                        // Collect and sort keys so wire order matches serde_json::Map.
                        let mut pairs: Vec<(&String, &Value<String>)> = m.iter().collect();
                        pairs.sort_unstable_by_key(|(k, _)| k.as_str());
                        if let Some(ref id_s) = id_str {
                            // Insert _id in sorted position among field keys.
                            // serde_json::Map sorts all keys including _id; we do the same.
                            let id_key = "_id";
                            let mut id_emitted = false;
                            for (k, v) in &pairs {
                                if !id_emitted && id_key < k.as_str() {
                                    map.serialize_entry(id_key, id_s)?;
                                    id_emitted = true;
                                }
                                map.serialize_entry(*k, *v)?;
                            }
                            if !id_emitted {
                                map.serialize_entry(id_key, id_s)?;
                            }
                        } else {
                            // No _id — just emit sorted field pairs.
                            for (k, v) in &pairs {
                                map.serialize_entry(*k, *v)?;
                            }
                        }
                        map.end()
                    }
                    _ => {
                        if let Some(ref id_s) = id_str {
                            // Non-map with id: emit {"_id": ..., "_value": ...} sorted.
                            let mut map = serializer.serialize_map(Some(2))?;
                            map.serialize_entry("_id", id_s)?;
                            map.serialize_entry("_value", fields)?;
                            map.end()
                        } else {
                            // Non-map without id: emit the value directly.
                            fields.serialize(serializer)
                        }
                    }
                }
            }
        }
    }
}

// ── Deserialize ────────────────────────────────────────────────────────────

impl<'de> Deserialize<'de> for InsertedRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Always decode as a serde_json::Value map. Clients only ever
        // read the Json variant; the Direct variant is a server-side
        // build optimisation.
        struct JsonVisitor;

        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = InsertedRecord;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut map = serde_json::Map::new();
                while let Some((k, v)) = access.next_entry::<String, serde_json::Value>()? {
                    map.insert(k, v);
                }
                Ok(InsertedRecord::Json(serde_json::Value::Object(map)))
            }
        }

        d.deserialize_map(JsonVisitor)
    }
}

// ── Conversions ────────────────────────────────────────────────────────────

impl From<serde_json::Value> for InsertedRecord {
    fn from(v: serde_json::Value) -> Self {
        InsertedRecord::Json(v)
    }
}

// ── Accessors ──────────────────────────────────────────────────────────────

impl InsertedRecord {
    /// Returns a `serde_json::Value` view of this record.
    ///
    /// For `Json` — borrows without allocation.
    /// For `Direct` — round-trips through `serde_json` (slow; only for
    /// code paths that explicitly need a `Value`, e.g. test assertions).
    pub fn as_json(&self) -> Cow<'_, serde_json::Value> {
        match self {
            InsertedRecord::Json(v) => Cow::Borrowed(v),
            InsertedRecord::Direct { .. } => {
                let bytes = serde_json::to_vec(self)
                    .expect("InsertedRecord::Direct serialize is infallible");
                Cow::Owned(
                    serde_json::from_slice(&bytes).expect("InsertedRecord round-trip is valid"),
                )
            }
        }
    }
}

// ── PartialEq (for tests) ──────────────────────────────────────────────────

impl PartialEq for InsertedRecord {
    fn eq(&self, other: &Self) -> bool {
        // Compare by serialised JSON value — works for both variants.
        let a = serde_json::to_value(self).ok();
        let b = serde_json::to_value(other).ok();
        a == b
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use shamir_types::types::common::TMap;

    fn make_direct() -> InsertedRecord {
        let mut map: TMap<String, Value<String>> = TMap::default();
        map.insert("name".to_string(), Value::Str("widget".to_string()));
        map.insert("qty".to_string(), Value::Int(42));
        let id = RecordId::system("test-id-00");
        InsertedRecord::Direct {
            id: Some(id),
            fields: QueryValue::Map(map),
        }
    }

    fn make_json(id_str: &str) -> InsertedRecord {
        // Field order must match what Direct emits: fields first, _id last.
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("widget"));
        map.insert("qty".to_string(), json!(42));
        map.insert("_id".to_string(), json!(id_str));
        InsertedRecord::Json(serde_json::Value::Object(map))
    }

    /// Wire-byte-identical: the msgpack emitted by `Direct` must equal
    /// the msgpack emitted by the equivalent `Json` value.
    #[test]
    fn inserted_record_byte_identical_to_json_value() {
        let direct = make_direct();
        // Get the id string the Direct variant will emit so we can build
        // a byte-identical Json counterpart.
        let id_str = if let InsertedRecord::Direct { id: Some(id), .. } = &direct {
            id.to_string()
        } else {
            unreachable!()
        };
        let json_rec = make_json(&id_str);

        let direct_bytes = rmp_serde::to_vec_named(&direct).expect("direct serialize");
        let json_bytes = rmp_serde::to_vec_named(&json_rec).expect("json serialize");

        assert_eq!(
            direct_bytes, json_bytes,
            "msgpack bytes must be identical for Direct and Json variants with same content"
        );
    }

    #[test]
    fn inserted_record_serde_json_round_trip() {
        let direct = make_direct();
        let id_str = if let InsertedRecord::Direct { id: Some(id), .. } = &direct {
            id.to_string()
        } else {
            unreachable!()
        };
        let json_val = serde_json::to_value(&direct).expect("serialize");
        assert_eq!(json_val["name"], "widget");
        assert_eq!(json_val["qty"], 42);
        assert_eq!(json_val["_id"], id_str);
    }

    #[test]
    fn inserted_record_as_json_direct() {
        let direct = make_direct();
        let id_str = if let InsertedRecord::Direct { id: Some(id), .. } = &direct {
            id.to_string()
        } else {
            unreachable!()
        };
        let v = direct.as_json();
        assert_eq!(v["name"], "widget");
        assert_eq!(v["_id"], id_str);
    }

    #[test]
    fn inserted_record_as_json_json() {
        let j = make_json("fake-id");
        // Borrowed — no allocation.
        let v = j.as_json();
        assert_eq!(v["qty"], 42);
    }
}
