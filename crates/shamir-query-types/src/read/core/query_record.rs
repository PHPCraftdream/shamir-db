//! QueryRecord — read-response row type.

use std::borrow::Cow;
use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_bytes::ByteBuf;
use shamir_types::types::value::QueryValue;

use num_bigint::BigInt;

use crate::write::InsertedRecord;

/// Read response row.
///
/// `Direct` is the canonical shape for all read-path and deserialized rows.
/// `Inserted` carries a write-result row from DML ops. `IdBytes` carries an
/// id-keyed row for `result_encoding = Id` responses.
///
/// Use the `get_value*` / `as_value` family for field access.
#[derive(Debug)]
pub enum QueryRecord {
    /// Projected row coming from the engine read path — zero extra
    /// allocation beyond the `QueryValue` produced by `project_value`.
    /// Also the canonical shape produced by the deserializer for all
    /// non-binary wire payloads (replaces the former legacy variant).
    Direct(QueryValue),
    /// A write-result row carried straight through from a DML op.
    ///
    /// Wraps the [`InsertedRecord`] that `execute_*` already built, so the
    /// batch layer can fold a `WriteResult` into a `QueryResult` WITHOUT
    /// re-materialising each row into an intermediate value.
    /// Serialization delegates to [`InsertedRecord`]'s impl.
    Inserted(InsertedRecord),
    /// A row returned id-keyed (no server de-intern); the client de-interns
    /// via its FieldMap. Emitted when the request set
    /// `result_encoding = Id`. The bytes are a single id-keyed msgpack map
    /// as produced by the engine's pass-through read path.
    ///
    /// Serializes as msgpack `bin` (binary) via [`ByteBuf`]; a deserializer
    /// receiving a msgpack `bin` value reconstructs this variant.
    IdBytes(ByteBuf),
}

impl Clone for QueryRecord {
    fn clone(&self) -> Self {
        match self {
            QueryRecord::Direct(v) => QueryRecord::Direct(v.clone()),
            QueryRecord::Inserted(rec) => QueryRecord::Inserted(rec.clone()),
            QueryRecord::IdBytes(b) => QueryRecord::IdBytes(b.clone()),
        }
    }
}

// ── serde ────────────────────────────────────────────────────────────────────

impl Serialize for QueryRecord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            QueryRecord::Direct(v) => v.serialize(s),
            QueryRecord::Inserted(rec) => rec.serialize(s),
            // ByteBuf's Serialize uses serde_bytes::serialize, emitting msgpack
            // `bin` (not a seq-of-u8) on the msgpack wire.
            QueryRecord::IdBytes(b) => b.serialize(s),
        }
    }
}

/// Visitor that routes msgpack `bin` to `IdBytes` and everything else to
/// `QueryValue`'s own visitor chain, wrapping the result in `Direct`.
///
/// `deserialize_any` is used so the deserializer advertises what type it holds
/// first; bytes map to `IdBytes`, all other self-describing types reconstruct
/// a `QueryValue` and wrap it in `Direct`.
struct QueryRecordVisitor;

impl<'de> Visitor<'de> for QueryRecordVisitor {
    type Value = QueryRecord;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a QueryRecord (map/value or binary bytes)")
    }

    // ── byte paths → IdBytes ─────────────────────────────────────────────────

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<QueryRecord, E> {
        Ok(QueryRecord::IdBytes(ByteBuf::from(v)))
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<QueryRecord, E> {
        Ok(QueryRecord::IdBytes(ByteBuf::from(v)))
    }

    // ── all other self-describing types → Direct(QueryValue) ─────────────────

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<QueryRecord, E> {
        Ok(QueryRecord::Direct(QueryValue::Bool(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<QueryRecord, E> {
        Ok(QueryRecord::Direct(QueryValue::Int(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<QueryRecord, E> {
        // Unified u64 contract (FG-1): values that fit in `i64` decode as
        // `QueryValue::Int`; values above `i64::MAX` promote losslessly to
        // `QueryValue::Big` (an arbitrary-precision `BigInt`) instead of being
        // silently clamped to `i64::MAX`.
        if v <= i64::MAX as u64 {
            Ok(QueryRecord::Direct(QueryValue::Int(v as i64)))
        } else {
            Ok(QueryRecord::Direct(QueryValue::Big(BigInt::from(v))))
        }
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<QueryRecord, E> {
        if !v.is_finite() {
            return Err(de::Error::custom("non-finite float in QueryRecord"));
        }
        Ok(QueryRecord::Direct(QueryValue::F64(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<QueryRecord, E> {
        Ok(QueryRecord::Direct(QueryValue::Str(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<QueryRecord, E> {
        Ok(QueryRecord::Direct(QueryValue::Str(v)))
    }

    fn visit_unit<E: de::Error>(self) -> Result<QueryRecord, E> {
        Ok(QueryRecord::Direct(QueryValue::Null))
    }

    fn visit_none<E: de::Error>(self) -> Result<QueryRecord, E> {
        Ok(QueryRecord::Direct(QueryValue::Null))
    }

    fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<QueryRecord, D2::Error> {
        Deserialize::deserialize(d)
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, seq: A) -> Result<QueryRecord, A::Error> {
        let v = QueryValue::deserialize(de::value::SeqAccessDeserializer::new(seq))?;
        Ok(QueryRecord::Direct(v))
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<QueryRecord, A::Error> {
        let v = QueryValue::deserialize(de::value::MapAccessDeserializer::new(map))?;
        Ok(QueryRecord::Direct(v))
    }
}

impl<'de> Deserialize<'de> for QueryRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(QueryRecordVisitor)
    }
}

// ── From conversions ──────────────────────────────────────────────────────────

impl From<QueryValue> for QueryRecord {
    fn from(v: QueryValue) -> Self {
        QueryRecord::Direct(v)
    }
}

impl From<InsertedRecord> for QueryRecord {
    fn from(r: InsertedRecord) -> Self {
        QueryRecord::Inserted(r)
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

impl QueryRecord {
    // ── QueryValue-native accessors ──────────────────────────────────────────

    /// View this row as a `QueryValue`.
    ///
    /// * `Direct(qv)` — `Cow::Borrowed(&qv)`: zero allocation, zero copy.
    /// * `Inserted(rec)` — `Cow::Owned(…)`: clones the record's `fields`
    ///   `QueryValue` (one clone).
    /// * `IdBytes(_)` — `Cow::Owned(QueryValue::Null)`: the bytes are an opaque
    ///   id-keyed msgpack blob; field-level access is not meaningful until the
    ///   client de-interns the keys.  Callers that need field access must
    ///   de-intern the bytes themselves.
    pub fn as_value(&self) -> Cow<'_, QueryValue> {
        match self {
            QueryRecord::Direct(qv) => Cow::Borrowed(qv),
            QueryRecord::Inserted(rec) => Cow::Owned(rec.fields.clone()),
            // IdBytes: opaque binary — field-level access is not meaningful
            // without client-side de-interning.  Return Null as a safe sentinel.
            QueryRecord::IdBytes(_) => Cow::Owned(QueryValue::Null),
        }
    }

    /// Look up a field in a `Direct` row by borrowing into the `QueryValue::Map`
    /// directly — zero allocation, no cache required.
    ///
    /// For `Inserted` / `IdBytes` this returns `None`.  Use
    /// [`get_value_owned`](Self::get_value_owned) when the variant is unknown.
    pub fn get_value(&self, key: &str) -> Option<&QueryValue> {
        match self {
            QueryRecord::Direct(qv) => qv.get(key),
            _ => None,
        }
    }

    /// Look up a field by name and return an owned `QueryValue`.
    ///
    /// Works for all variants:
    /// * `Direct` — borrows into the map and clones the value found (one clone).
    /// * `Inserted` — converts to `QueryValue` via [`as_value`](Self::as_value)
    ///   then looks up the key.
    /// * `IdBytes` — always `None`.
    pub fn get_value_owned(&self, key: &str) -> Option<QueryValue> {
        match self {
            QueryRecord::Direct(qv) => qv.get(key).cloned(),
            QueryRecord::IdBytes(_) => None,
            _ => {
                let v = self.as_value();
                v.get(key).cloned()
            }
        }
    }

    /// Look up a string field using `QueryValue`-native access.
    ///
    /// * `Direct` — borrows from the inner `QueryValue::Map` (zero allocation).
    /// * `Inserted` — borrows from the `fields: QueryValue` in the
    ///   `InsertedRecord` (also zero allocation, borrow from `self`).
    /// * `IdBytes` — always `None` (opaque bytes; no field access).
    pub fn get_value_str(&self, key: &str) -> Option<&str> {
        match self {
            QueryRecord::Direct(qv) => qv.get(key).and_then(QueryValue::as_str),
            QueryRecord::Inserted(rec) => rec.fields.get(key).and_then(QueryValue::as_str),
            QueryRecord::IdBytes(_) => None,
        }
    }

    /// Look up an `i64` field using `QueryValue`-native access.
    ///
    /// Returns `None` if the key is absent or the value is not `QueryValue::Int`.
    pub fn get_value_i64(&self, key: &str) -> Option<i64> {
        match self {
            QueryRecord::Direct(qv) => qv.get(key).and_then(QueryValue::as_i64),
            QueryRecord::Inserted(_) => self
                .get_value_owned(key)
                .as_ref()
                .and_then(QueryValue::as_i64),
            QueryRecord::IdBytes(_) => None,
        }
    }

    /// Look up a `u64` field using `QueryValue`-native access.
    ///
    /// Returns `None` if the key is absent, the value is not `QueryValue::Int`,
    /// or the integer is negative.
    pub fn get_value_u64(&self, key: &str) -> Option<u64> {
        match self {
            QueryRecord::Direct(qv) => qv.get(key).and_then(QueryValue::as_u64),
            QueryRecord::Inserted(_) => self
                .get_value_owned(key)
                .as_ref()
                .and_then(QueryValue::as_u64),
            QueryRecord::IdBytes(_) => None,
        }
    }

    /// Look up a `bool` field using `QueryValue`-native access.
    ///
    /// Returns `None` if the key is absent or the value is not `QueryValue::Bool`.
    pub fn get_value_bool(&self, key: &str) -> Option<bool> {
        match self {
            QueryRecord::Direct(qv) => qv.get(key).and_then(QueryValue::as_bool),
            QueryRecord::Inserted(_) => self
                .get_value_owned(key)
                .as_ref()
                .and_then(QueryValue::as_bool),
            QueryRecord::IdBytes(_) => None,
        }
    }
}

// ── PartialEq (for tests) ─────────────────────────────────────────────────────

impl PartialEq for QueryRecord {
    fn eq(&self, other: &Self) -> bool {
        // Fast-path: both IdBytes — compare raw bytes directly.
        if let (QueryRecord::IdBytes(a), QueryRecord::IdBytes(b)) = (self, other) {
            return a == b;
        }
        // General case: normalise via QueryValue so Direct == Inserted works.
        self.as_value() == other.as_value()
    }
}
