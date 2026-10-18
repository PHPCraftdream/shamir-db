//! The zero-copy borrowing lens over canonical (id-keyed) MessagePack record
//! bytes produced by `InnerValue::to_bytes()`. One primary export:
//! [`RecordView`].
//!
//! The storage codec serialises `InternerKey` map keys as msgpack **`bin`**
//! (variable-width little-endian id bytes via `InternerKey::serialize` →
//! `serialize_bytes`). The lens matches keys by encoding the target field's
//! `u64` id to the same LE wire bytes (a la `eval_bytes::interned_key_bytes`)
//! and comparing against each `bin` key in the map.
//!
//! ## Marker coverage
//! The lens handles EXACTLY the marker set the production encoder emits and
//! the canonical decoder accepts: all int widths (`FixPos`/`FixNeg`/`U8`..
//! `U64`/`I8`..`I64`), `F32`/`F64`, `FixStr`/`Str8`/`Str16`/`Str32`,
//! `Bin8`/`Bin16`/`Bin32`, `FixArray`/`Array16`/`Array32`,
//! `FixMap`/`Map16`/`Map32`, all ext (`FixExt1`/`FixExt2`/`FixExt4`/
//! `FixExt8`/`FixExt16`/`Ext8`/`Ext16`/`Ext32`), `Null`/`True`/`False`.
//! Reserved markers return [`RecordViewError::ReservedMarker`].
//!
//! ## Untrusted-input safety
//! Every primitive returns [`Result`] / [`Option`] and bounds-checks every
//! read. The lens NEVER panics on malformed/truncated bytes — it surfaces a
//! [`RecordViewError`] (or `None` for single-field misses, matching the
//! `Option` return of [`RecordView::get`]).
//!
//! ## Forward compatibility
//! The API is shaped to sit behind the Stage-2 `RecordRef` trait
//! (`get` / `fields` / `get_path`) without reshaping — both `InnerValue` and
//! `RecordView` will implement it, with static dispatch.

mod cursor;
mod error;
mod field_index;
mod read_value;
mod skip;
mod view;

pub use error::{RecordViewError, MAX_MSGPACK_DEPTH};
pub use field_index::FieldIndex;
pub(crate) use read_value::read_value;
pub(crate) use skip::skip_value;
pub use view::RecordView;
