//! MessagePack codec with on-the-fly key interning
//!
//! This module provides MessagePack encoding/decoding directly to/from InnerValue
//! without using UserValue (which is deprecated and for tests only).
//!
//! §5b floor (#61): this is the storage codec — the home of `InnerValue`
//! as the id-keyed wire/storage form, NOT a hot-path materialization. See
//! `docs/dev-artifacts/perf/innervalue-floor.md` (Category 1 — type library).

mod decode;
mod encode;
mod merge;

pub use decode::{msgpack_to_inner, msgpack_to_inner_zerocopy};
pub use encode::{
    inner_to_msgpack, query_value_to_storage_bytes, query_value_to_storage_bytes_into,
};
pub use merge::merge_storage_bytes;

pub(crate) use merge::write_map_header;

#[cfg(test)]
pub(crate) use decode::msgpack_to_inner_legacy;
