//! Allocation-free write-result record for INSERT/UPSERT hot paths.

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{QueryValue, Value};

/// A single record returned inside [`WriteResult`](super::WriteResult).
///
/// Wire shape is a msgpack map of fields plus `_id` — built directly from
/// `QueryValue` + `RecordId` without allocating an intermediate map.
///
/// * `id` — when `Some`, `_id` is injected in sorted-key position during
///   serialization. When `None`, only the `fields` map is serialized — used
///   by UPDATE-RETURNING and SET-UPDATE where the wire result carries no `_id`.
#[derive(Debug, Clone)]
pub struct InsertedRecord {
    /// Optional record id. When `Some`, `_id` is injected in sorted-key
    /// position during serialization.
    pub id: Option<RecordId>,
    /// The record's field data as a `QueryValue` (typically a `Map`).
    pub fields: QueryValue,
}

// ── Serialize ──────────────────────────────────────────────────────────────

impl Serialize for InsertedRecord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let InsertedRecord { id, fields } = self;
        // Emit in sorted-key order so wire bytes are deterministic.
        let id_str = id.as_ref().map(|r| r.to_string());
        match fields {
            Value::Map(m) => {
                let extra = if id_str.is_some() { 1 } else { 0 };
                let field_count = m.len() + extra;
                let mut map = serializer.serialize_map(Some(field_count))?;
                // Collect and sort keys so wire order is deterministic.
                let mut pairs: Vec<(&String, &Value<String>)> = m.iter().collect();
                pairs.sort_unstable_by_key(|(k, _)| k.as_str());
                if let Some(ref id_s) = id_str {
                    // Insert _id in sorted position among field keys.
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

// ── Deserialize ────────────────────────────────────────────────────────────

impl<'de> Deserialize<'de> for InsertedRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Decode as a `QueryValue` map (byte-identical wire shape).
        // The `_id` key is stored in `fields` when present; callers can
        // look it up via `get_value_owned("_id")`.
        struct DirectVisitor;

        impl<'de> Visitor<'de> for DirectVisitor {
            type Value = InsertedRecord;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                use serde::de::value::MapAccessDeserializer;
                let fields = QueryValue::deserialize(MapAccessDeserializer::new(map))?;
                Ok(InsertedRecord { id: None, fields })
            }
        }

        d.deserialize_map(DirectVisitor)
    }
}

// ── Accessors ──────────────────────────────────────────────────────────────

impl InsertedRecord {
    /// Look up a field by name and return an owned `QueryValue`.
    ///
    /// Zero-allocation lookup into the `QueryValue::Map`. The synthetic `_id`
    /// field (stored separately in `id: Option<RecordId>`) is returned as a
    /// `QueryValue::Str` from `id` when present.
    pub fn get_value_owned(&self, key: &str) -> Option<QueryValue> {
        if key == "_id" {
            if let Some(id) = &self.id {
                return Some(QueryValue::Str(id.to_string()));
            }
            return self.fields.get(key).cloned();
        }
        self.fields.get(key).cloned()
    }
}

// ── PartialEq (for tests) ──────────────────────────────────────────────────

impl PartialEq for InsertedRecord {
    fn eq(&self, other: &Self) -> bool {
        // Compare id and fields directly.  Fields are IndexMap-backed so map
        // equality is key-set + value equality (insertion order independent).
        self.id == other.id && self.fields == other.fields
    }
}
