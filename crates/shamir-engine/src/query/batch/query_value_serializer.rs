//! Direct `T: Serialize` → [`QueryValue`] serializer (F5).
//!
//! Replaces the per-ForEach-iteration / per-sub-batch
//! `rmp_serde::to_vec_named` → `from_slice::<QueryValue>` round-trip that
//! used to live in `query_runner.rs`. Instead of encoding to a msgpack byte
//! buffer and immediately re-parsing those same bytes, this drives the
//! *existing* `Serialize` impls of `TMap<String, QueryResult>` /
//! `QueryRecord` / `InsertedRecord` / `QueryStats` / `PaginationInfo` /
//! `ExplainPlan` / `Value` into an in-memory `QueryValue` tree — one pass,
//! no intermediate byte buffer, no re-parse.
//!
//! # Why a `Serializer`, not a hand-mirroring converter
//!
//! `QueryRecord` has three variants with non-obvious wire behaviour:
//! - `Inserted(rec)` delegates to `InsertedRecord::serialize`, which
//!   interleaves a synthetic `"_id"` key in **sorted position** among the
//!   field keys — not "fields + appended _id".
//! - `IdBytes(b)` serializes as msgpack `bin` (round-trips to
//!   `QueryValue::Bin`, *not* `Null` as `QueryRecord::as_value()` returns).
//! - `Direct(v)` is a plain pass-through to `Value::serialize`.
//!
//! A hand-written per-variant converter would have to re-implement the first
//! two and silently drift whenever they change. By redirecting the *same*
//! `Serialize` calls to this serializer, every existing impl runs verbatim
//! — the sorted-`_id` logic stays in one place, bytes stay bytes, and any
//! future field added to any of these types flows through automatically.
//!
//! # Wire-shape parity with the old msgpack round-trip
//!
//! The old encoder was `rmp_serde::to_vec_named`, so structs serialise as
//! *maps* (field-name keys). This serializer mirrors that by routing
//! `serialize_struct` through the same map builder as `serialize_map`.
//! `Value::Dec` / `Value::Big` become `Value::Str` (their `Serialize` emits a
//! string), and `Value::Set` becomes `Value::List` — exactly what the old
//! `ValueVisitor` produced after re-parsing the bytes. The exhaustive
//! differential test in `tests/query_value_serializer_tests.rs` asserts
//! `PartialEq` parity across every representative shape.
//!
//! `ValueVisitor::visit_map` also applies a key-prefix coercion convention
//! (`"i:field"`, `"dec:field"`, …) on the *deserialise* side. Engine-built
//! `TMap<String, QueryResult>` data uses plain field names (already coerced
//! at read time), so this serializer builds `Value::Map` entries verbatim and
//! matches the round-trip for all realistic inputs; the convention never
//! fires for the plain-keyed struct-field / alias / record-field names this
//! conversion handles.

use std::fmt;

use serde::ser::{
    self, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeTuple,
    SerializeTupleStruct, Serializer,
};
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::QueryValue;

/// Convert any `Serialize` into a [`QueryValue`] by driving its `Serialize`
/// impl through [`QueryValueSerializer`].
///
/// Returns `Err` only if the value invokes a `Serializer` method this
/// serializer does not support — a provably-unreachable path for the
/// `TMap<String, QueryResult>` shapes this is called with (see the module
/// docs). Callers `.ok()` to match the old `Option<QueryValue>` shape.
pub(crate) fn to_query_value<T: Serialize + ?Sized>(value: &T) -> Result<QueryValue, QvSerError> {
    value.serialize(QueryValueSerializer)
}

// ── error ───────────────────────────────────────────────────────────────────

/// Error raised by [`QueryValueSerializer`]. Only ever produced on a
/// provably-unreachable `Serializer` method (tuple/struct variants) or a
/// contract violation (a non-string map key, which the
/// `TMap<String, QueryResult>` tree never produces).
#[derive(Debug)]
pub(crate) struct QvSerError(pub String);

impl fmt::Display for QvSerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for QvSerError {}

impl ser::Error for QvSerError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        QvSerError(msg.to_string())
    }
}

// ── serializer ──────────────────────────────────────────────────────────────

/// Zero-sized [`Serializer`] whose `Ok` type is [`QueryValue`].
///
/// Construct a fresh `QueryValueSerializer` for each (recursive) `serialize`
/// call — it carries no state, so the copy is free. Private: only
/// [`to_query_value`] constructs it; the builder associated types (`QvSeq` /
/// `QvMap` / `Unreachable`) are impl details that stay private alongside it.
struct QueryValueSerializer;

impl Serializer for QueryValueSerializer {
    type Ok = QueryValue;
    type Error = QvSerError;

    type SerializeSeq = QvSeq;
    type SerializeTuple = QvSeq;
    type SerializeTupleStruct = QvSeq;
    type SerializeTupleVariant = Unreachable;
    type SerializeMap = QvMap;
    type SerializeStruct = QvMap;
    type SerializeStructVariant = Unreachable;

    // ── primitives ──────────────────────────────────────────────────────────

    fn serialize_bool(self, v: bool) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Bool(v))
    }

    fn serialize_i8(self, v: i8) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Int(v as i64))
    }
    fn serialize_i16(self, v: i16) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Int(v as i64))
    }
    fn serialize_i32(self, v: i32) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Int(v as i64))
    }
    fn serialize_i64(self, v: i64) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Int(v))
    }

    // All unsigned ints land in `Value::Int` — matches `ValueVisitor::visit_u64`
    // (`value as i64`) and `visit_i64` after msgpack's fixint re-encoding.
    fn serialize_u8(self, v: u8) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Int(v as i64))
    }
    fn serialize_u16(self, v: u16) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Int(v as i64))
    }
    fn serialize_u32(self, v: u32) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Int(v as i64))
    }
    fn serialize_u64(self, v: u64) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Int(v as i64))
    }

    fn serialize_f32(self, v: f32) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::F64(v as f64))
    }
    fn serialize_f64(self, v: f64) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::F64(v))
    }

    fn serialize_char(self, v: char) -> Result<QueryValue, QvSerError> {
        // serde treats `char` as a length-1 string; the round-trip lands in
        // `Value::Str` via `visit_str`.
        Ok(QueryValue::Str(v.to_string()))
    }

    fn serialize_str(self, v: &str) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Str(v.to_owned()))
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<QueryValue, QvSerError> {
        // `QueryRecord::IdBytes(ByteBuf)` reaches here → `Value::Bin`, matching
        // the msgpack `bin` token → `visit_bytes` → `Value::Bin` round-trip.
        Ok(QueryValue::Bin(v.to_vec()))
    }

    // ── null-ish ────────────────────────────────────────────────────────────

    fn serialize_none(self) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Null)
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<QueryValue, QvSerError> {
        value.serialize(QueryValueSerializer)
    }

    fn serialize_unit(self) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::Null)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<QueryValue, QvSerError> {
        // `to_vec_named` emits a unit variant as a bare string; on re-parse
        // `ValueVisitor::visit_str` reconstructs `Value::Str`. (`PlanType`'s
        // variants reach here when `explain` is present.)
        Ok(QueryValue::Str(variant.to_owned()))
    }

    // ── newtype ─────────────────────────────────────────────────────────────

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<QueryValue, QvSerError> {
        // Transparent — matches `to_vec_named`'s newtype-struct encoding.
        value.serialize(QueryValueSerializer)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<QueryValue, QvSerError> {
        // `to_vec_named` emits `{ variant: inner }`; re-parse → single-entry Map.
        let mut map: TMap<String, QueryValue> = new_map_wc(1);
        map.insert(variant.to_owned(), value.serialize(QueryValueSerializer)?);
        Ok(QueryValue::Map(map))
    }

    // ── seq / tuple / tuple-struct ──────────────────────────────────────────

    fn serialize_seq(self, len: Option<usize>) -> Result<QvSeq, QvSerError> {
        Ok(QvSeq {
            items: Vec::with_capacity(len.unwrap_or(0)),
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<QvSeq, QvSerError> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<QvSeq, QvSerError> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Unreachable, QvSerError> {
        Err(QvSerError(format!(
            "QueryValueSerializer: tuple_variant `{variant}` is not supported \
             (no type in the TMap<String, QueryResult> tree uses one)"
        )))
    }

    // ── map / struct ────────────────────────────────────────────────────────

    fn serialize_map(self, len: Option<usize>) -> Result<QvMap, QvSerError> {
        Ok(QvMap {
            pairs: Vec::with_capacity(len.unwrap_or(0)),
            pending_key: None,
        })
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<QvMap, QvSerError> {
        // `to_vec_named` encodes structs as maps (field-name keys) — route
        // through the same builder so the re-parsed shape matches exactly.
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Unreachable, QvSerError> {
        Err(QvSerError(format!(
            "QueryValueSerializer: struct_variant `{variant}` is not supported \
             (no type in the TMap<String, QueryResult> tree uses one)"
        )))
    }
}

// ── seq builder ─────────────────────────────────────────────────────────────

struct QvSeq {
    items: Vec<QueryValue>,
}

impl SerializeSeq for QvSeq {
    type Ok = QueryValue;
    type Error = QvSerError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), QvSerError> {
        self.items.push(value.serialize(QueryValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<QueryValue, QvSerError> {
        Ok(QueryValue::List(self.items))
    }
}

impl SerializeTuple for QvSeq {
    type Ok = QueryValue;
    type Error = QvSerError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), QvSerError> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<QueryValue, QvSerError> {
        SerializeSeq::end(self)
    }
}

impl SerializeTupleStruct for QvSeq {
    type Ok = QueryValue;
    type Error = QvSerError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), QvSerError> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<QueryValue, QvSerError> {
        SerializeSeq::end(self)
    }
}

// ── map / struct builder ────────────────────────────────────────────────────

struct QvMap {
    pairs: Vec<(String, QueryValue)>,
    pending_key: Option<String>,
}

impl QvMap {
    fn finish(self) -> QueryValue {
        let mut map: TMap<String, QueryValue> = new_map_wc(self.pairs.len());
        for (k, v) in self.pairs {
            map.insert(k, v);
        }
        QueryValue::Map(map)
    }
}

impl SerializeMap for QvMap {
    type Ok = QueryValue;
    type Error = QvSerError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), QvSerError> {
        let k = key.serialize(QueryValueSerializer)?;
        self.pending_key = Some(key_to_string(k)?);
        Ok(())
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), QvSerError> {
        let v = value.serialize(QueryValueSerializer)?;
        match self.pending_key.take() {
            Some(k) => {
                self.pairs.push((k, v));
                Ok(())
            }
            None => Err(QvSerError(
                "QueryValueSerializer: serialize_value called before serialize_key".to_string(),
            )),
        }
    }

    fn end(self) -> Result<QueryValue, QvSerError> {
        Ok(self.finish())
    }
}

impl SerializeStruct for QvMap {
    type Ok = QueryValue;
    type Error = QvSerError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), QvSerError> {
        let v = value.serialize(QueryValueSerializer)?;
        self.pairs.push((key.to_owned(), v));
        Ok(())
    }

    fn skip_field(&mut self, _key: &'static str) -> Result<(), QvSerError> {
        Ok(())
    }

    fn end(self) -> Result<QueryValue, QvSerError> {
        Ok(self.finish())
    }
}

/// Convert a serialized map key into the `String` a `Value<String>` map
/// requires. Every key in the `TMap<String, QueryResult>` tree is a string
/// (alias names, serde struct field names, record field names, `"_id"`); a
/// non-string key is a contract violation, not a runtime input.
fn key_to_string(k: QueryValue) -> Result<String, QvSerError> {
    match k {
        QueryValue::Str(s) => Ok(s),
        other => Err(QvSerError(format!(
            "QueryValueSerializer: map key must be a string, got {other:?}"
        ))),
    }
}

// ── unreachable variant builders ────────────────────────────────────────────

/// Placeholder for tuple/struct *variant* serialization — no type in the
/// `TMap<String, QueryResult>` tree produces one, so every method errors so
/// a future type addition surfaces loudly instead of silently emitting a
/// wrong shape.
struct Unreachable;

impl ser::SerializeTupleVariant for Unreachable {
    type Ok = QueryValue;
    type Error = QvSerError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, _value: &T) -> Result<(), QvSerError> {
        Err(QvSerError(
            "QueryValueSerializer: SerializeTupleVariant reached (unreachable)".to_string(),
        ))
    }

    fn end(self) -> Result<QueryValue, QvSerError> {
        Err(QvSerError(
            "QueryValueSerializer: SerializeTupleVariant::end reached (unreachable)".to_string(),
        ))
    }
}

impl ser::SerializeStructVariant for Unreachable {
    type Ok = QueryValue;
    type Error = QvSerError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        _value: &T,
    ) -> Result<(), QvSerError> {
        Err(QvSerError(
            "QueryValueSerializer: SerializeStructVariant reached (unreachable)".to_string(),
        ))
    }

    fn end(self) -> Result<QueryValue, QvSerError> {
        Err(QvSerError(
            "QueryValueSerializer: SerializeStructVariant::end reached (unreachable)".to_string(),
        ))
    }
}
