//! Test-only support surface for `shamir-lang`.
//!
//! NOT A PRODUCTION API. This module exists so the host crate's execution
//! tests (`shamir-wasm-host`, which takes `shamir-lang` as a dev-dependency)
//! can build the M1 runtime module and its codec vectors without duplicating
//! the emitters. It is `#[doc(hidden)]` and excluded from any stability
//! promise.
//!
//! Design deviation (documented in the M1 final report): the design's
//! `cfg(test)`-only placement is impossible cross-crate (a `cfg(test)`
//! module is invisible to a dev-dependency, and a `#[path]` include of
//! sibling sources would fight the no-local-use and layout rules), so
//! `testsupport` ships behind an explicit `testsupport` Cargo feature
//! instead, enabled only by the host crate's dev-dependency.

mod fixtures;
mod msgpack;
mod vectors;
mod wasm;

#[cfg(all(test, feature = "testsupport"))]
mod tests;

pub use fixtures::{admission_policy_64kib, default_caps, tiny_caps};
pub use msgpack::{
    enc_array, enc_bin, enc_bool, enc_call_envelope, enc_f64, enc_i64, enc_map_sorted, enc_nil,
    enc_str, key_payload,
};
pub use vectors::{codec_vectors, CodecVector, VectorExpectation};
pub use wasm::{compile_with_references, runtime_module};
