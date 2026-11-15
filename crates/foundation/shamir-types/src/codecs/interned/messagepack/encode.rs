//! MessagePack encoders: streaming `Serialize` wrappers over `rmp_serde`.

use crate::codecs::CodecError;
use crate::core::interner::{Interner, InternerKey};
use crate::types::value::{InnerValue, QueryValue, Value};
use bytes::Bytes;
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

/// Encodes InnerValue to MessagePack bytes, de-interning keys.
///
/// Streams directly into the output buffer via `rmp_serde::to_vec`
/// over an `InternedRef` wrapper. No intermediate `rmpv::Value`
/// tree is built — map keys are de-interned to `&str` and written
/// in-place when each Map entry is serialised.
pub fn inner_to_msgpack(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
    rmp_serde::to_vec(&InternedRef { value, interner })
        .map_err(|e| CodecError::Encode(format!("MessagePack encode error: {}", e)))
}

/// Borrowed view of an `InnerValue` paired with the `Interner` used
/// to resolve its map keys. Implements `Serialize` so `rmp_serde` (or
/// any other serde format) can write it without an intermediate value
/// tree.
struct InternedRef<'a> {
    value: &'a InnerValue,
    interner: &'a Interner,
}

impl Serialize for InternedRef<'_> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self.value {
            Value::Null => ser.serialize_unit(),
            Value::Bool(b) => ser.serialize_bool(*b),
            Value::Int(i) => ser.serialize_i64(*i),
            Value::F64(f) => ser.serialize_f64(*f),
            Value::Dec(d) => ser.serialize_str(&d.to_string()),
            Value::Big(b) => ser.serialize_str(&b.to_string()),
            Value::Str(s) => ser.serialize_str(s),
            Value::Bin(b) => ser.serialize_bytes(b),
            Value::List(l) => {
                let mut seq = ser.serialize_seq(Some(l.len()))?;
                for el in l {
                    seq.serialize_element(&InternedRef {
                        value: el,
                        interner: self.interner,
                    })?;
                }
                seq.end()
            }
            Value::Set(s) => {
                let mut seq = ser.serialize_seq(Some(s.len()))?;
                for el in s {
                    seq.serialize_element(&InternedRef {
                        value: el,
                        interner: self.interner,
                    })?;
                }
                seq.end()
            }
            Value::Map(m) => {
                let mut map = ser.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    // Borrow the interned key string without allocating
                    // a `UserKey(String)` per entry (hunt #7). The
                    // `with_str` closure scope covers the
                    // `serialize_entry` call so the `&str` stays live.
                    let r: Result<(), S::Error> = self
                        .interner
                        .with_str(k, |key_str| {
                            map.serialize_entry(
                                key_str,
                                &InternedRef {
                                    value: v,
                                    interner: self.interner,
                                },
                            )
                        })
                        .ok_or_else(|| {
                            serde::ser::Error::custom(format!(
                                "Interned key not found in interner: {:?}",
                                k
                            ))
                        })?;
                    r?;
                }
                map.end()
            }
        }
    }
}

// ============================================================================
// W2d: direct QueryValue -> id-keyed storage msgpack encoder
//
// Streaming Serialize that mirrors `Value<InternerKey>::serialize`
// (value.rs:61-98) marker-for-marker for every value variant, but interns each
// map key on the fly from the source `QueryValue` (string-keyed) and writes the
// key as `InternerKey::serialize` (minimal-width LE bin) — IDENTICAL to the
// reference path `query_value_to_inner_with(qv, f).to_bytes()`. The wire bytes
// are the storage format (WAL body verbatim, recovery decode target).
// ============================================================================

/// Encodes a `QueryValue` (string-keyed) directly to id-keyed storage MessagePack
/// bytes, interning each map key via `intern_key`.
///
/// This is the byte-identical fast path for the write cutover (W2d): it produces
/// exactly the same bytes as
/// `query_value_to_inner_with(qv, intern_key).unwrap().to_bytes().unwrap()`,
/// but without materialising the intermediate `InnerValue` tree — map keys are
/// interned and streamed straight into the output buffer.
///
/// The map KEY is serialised through [`InternerKey::serialize`] (minimal-width
/// LE bytes via `serialize_bytes`), matching [`Value::serialize`] for the
/// `Value<InternerKey>::Map` arm (value.rs:89-95). The source map's insertion
/// order is preserved.
pub fn query_value_to_storage_bytes<F>(qv: &QueryValue, intern_key: &F) -> Result<Bytes, CodecError>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    let wrapper = QvInternedRef {
        value: qv,
        intern: intern_key,
    };
    rmp_serde::to_vec(&wrapper)
        .map(Bytes::from)
        .map_err(|e| CodecError::Encode(format!("MessagePack encode error: {}", e)))
}

/// Scratch-buffer variant of [`query_value_to_storage_bytes`].
///
/// Serialises `qv` into the provided `scratch` buffer (which is **cleared**
/// first), then returns an owned `Bytes` via zero-copy `Bytes::from(Vec)`.
/// The Vec is consumed (moved into Bytes without memcpy); `scratch` is left
/// empty and re-grows on the next call. The caller keeps ownership of `scratch`
/// for the loop variable; the key win over a bare `rmp_serde::to_vec` call is
/// that the function signature signals batch intent and the caller can
/// pre-allocate with `Vec::with_capacity`.
///
/// Compared to the previous `Bytes::copy_from_slice` variant, this eliminates
/// the +1 alloc + memcpy per row that was causing the L12 regression on N=1.
pub fn query_value_to_storage_bytes_into<F>(
    qv: &QueryValue,
    intern_key: &F,
    scratch: &mut Vec<u8>,
) -> Result<Bytes, CodecError>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    scratch.clear();
    let wrapper = QvInternedRef {
        value: qv,
        intern: intern_key,
    };
    rmp_serde::encode::write(&mut *scratch, &wrapper)
        .map_err(|e| CodecError::Encode(format!("MessagePack encode error: {}", e)))?;
    // Zero-copy: Bytes::from(Vec) takes ownership of the Vec's heap allocation
    // without memcpy (the Vec becomes the Bytes' backing store).
    // std::mem::take leaves `scratch` as an empty Vec (cap=0); the next
    // iteration's `write()` will re-allocate via Vec's doubling strategy.
    Ok(Bytes::from(std::mem::take(scratch)))
}

/// Borrowed `QueryValue` paired with a key-interning closure. Implements
/// `Serialize` so `rmp_serde` can stream it straight to the output buffer.
///
/// For every variant except `Map`, this mirrors
/// `Value<InternerKey>::serialize` (value.rs:61-98) exactly. For the `Map` arm
/// it iterates the source map in insertion order and, for each `(key_str, val)`,
/// interns `key_str` and serialises the resulting `InternerKey` as the entry key
/// (which dispatches to [`InternerKey::serialize`] → minimal-width LE bin),
/// matching the reference path's map-key encoding.
struct QvInternedRef<'a, F>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    value: &'a QueryValue,
    intern: &'a F,
}

impl<F> Serialize for QvInternedRef<'_, F>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.serialize_inner(ser)
    }
}

impl<F> QvInternedRef<'_, F>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    fn serialize_inner<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.value {
            Value::Null => ser.serialize_unit(),
            Value::Bool(b) => ser.serialize_bool(*b),
            Value::Int(i) => ser.serialize_i64(*i),
            Value::F64(f) => ser.serialize_f64(*f),
            Value::Dec(d) => ser.serialize_str(&d.to_string()),
            Value::Big(b) => ser.serialize_str(&b.to_string()),
            Value::Str(s) => ser.serialize_str(s),
            Value::Bin(b) => ser.serialize_bytes(b),
            Value::List(l) => {
                let mut seq = ser.serialize_seq(Some(l.len()))?;
                for el in l {
                    seq.serialize_element(&Self::wrap(el, self.intern))?;
                }
                seq.end()
            }
            Value::Set(s) => {
                let mut seq = ser.serialize_seq(Some(s.len()))?;
                for el in s {
                    seq.serialize_element(&Self::wrap(el, self.intern))?;
                }
                seq.end()
            }
            Value::Map(m) => {
                let mut map = ser.serialize_map(Some(m.len()))?;
                for (key_str, val) in m {
                    let ik = (self.intern)(key_str).map_err(serde::ser::Error::custom)?;
                    let entry_val = Self::wrap(val, self.intern);
                    map.serialize_entry(&ik, &entry_val)?;
                }
                map.end()
            }
        }
    }

    #[inline]
    fn wrap<'b>(value: &'b QueryValue, intern: &'b F) -> QvInternedRef<'b, F> {
        QvInternedRef { value, intern }
    }
}
