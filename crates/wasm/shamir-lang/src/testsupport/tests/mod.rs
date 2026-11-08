//! Unit tests for the codec vectors: an independent iterative reference
//! decoder (`reference.rs`) plus round-trip assertions over every vector.

mod reference;
mod vectors_tests;

pub(super) use reference::{decode_reference, enc_value};
