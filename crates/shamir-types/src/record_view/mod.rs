//! Zero-copy borrowing lens over canonical (id-keyed) MessagePack record
//! bytes — the Stage-1 primitive of the RecordView migration
//! (see `docs/dev-artifacts/perf/record-view-migration.md` §2, §3, §8, §9).
//!
//! **ADDITIVE — not wired into the engine.** Stage 2 puts this behind a
//! `RecordRef` trait; Stages 3-4 migrate consumers. Nothing in this crate
//! changes behaviour as a result of this module existing.
//!
//! # What this is
//! [`RecordView`] is a thin cursor (`&[u8]`) that reads fields *on demand*
//! directly from the **storage-form** bytes (`InnerValue::to_bytes()`),
//! decoding only the marker of the matched field — it never materialises an
//! `InnerValue` tree. Per the Stage-0 GO numbers, this is ~100-150× cheaper
//! than a full tree decode for the common "read / match ONE field" case
//! (filter / index-extract / projection / validators touch 1-3 fields of a
//! many-field record).
//!
//! # Storage form
//! The storage codec (`InnerValue::to_bytes()` = `rmp_serde::to_vec`) maps
//! `InternerKey` keys via `InternerKey::serialize` → `serialize_bytes` →
//! msgpack **`bin`** (variable-width little-endian id bytes, 1/2/4/8 bytes).
//! Map keys in storage are therefore `bin` markers (0xc4/0xc5/0xc6), NOT
//! `str` markers. The lens matches keys by encoding the target field's
//! `InternerKey` (or raw `u64` id) to the same variable-width LE bytes and
//! comparing against each `bin` key — exactly as
//! `eval_bytes::interned_key_bytes` does.
//!
//! # Marker coverage
//! The lens handles EXACTLY the marker set the production encoder emits and
//! the canonical decoder accepts: all int widths, `F32`/`F64`,
//! `FixStr`/`Str8`/`Str16`/`Str32`, `Bin8`/`Bin16`/`Bin32`,
//! `FixArray`/`Array16`/`Array32`, `FixMap`/`Map16`/`Map32`, all ext types,
//! `Null`/`True`/`False`. Reserved markers error.
//! See [`lens`] for the full primitive surface.
//!
//! # Untrusted-input safety
//! Every primitive returns `Result` / `Option` and bounds-checks every read.
//! The lens NEVER panics on malformed/truncated bytes.

mod kind;
mod lens;
mod record_ref;
mod record_value;
mod scalar_ref;

pub use kind::Kind;
pub(crate) use lens::skip_value;
pub use lens::{FieldIndex, RecordView, RecordViewError, MAX_MSGPACK_DEPTH};
pub use record_ref::{HavingView, RecordRef};
pub use record_value::{RawSeq, RawSeqIter, RecordValue};
pub use scalar_ref::{scalar_ref_cmp, scalar_ref_cmp_qv, ScalarRef};

#[cfg(test)]
mod tests;
