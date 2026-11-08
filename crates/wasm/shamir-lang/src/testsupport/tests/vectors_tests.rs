//! Round-trip assertions over every codec vector: each Accepted vector's
//! raw input bytes must decode through the independent reference decoder and
//! re-encode to exactly the canonical bytes, and each Rejected vector must
//! make the reference decoder raise.
use super::{decode_reference, enc_value};
use crate::testsupport::vectors::{codec_vectors, VectorExpectation};

#[test]
fn accepted_vectors_decode_and_reencode_to_canonical_bytes() {
    for vector in &codec_vectors() {
        let VectorExpectation::Accepted(canonical) = &vector.expectation else {
            continue;
        };
        let value = decode_reference(&vector.input)
            .unwrap_or_else(|error| panic!("vector {} must decode: {error}", vector.name));
        assert_eq!(
            &enc_value(&value),
            canonical,
            "vector {} re-encodes to canonical bytes",
            vector.name
        );
    }
}

#[test]
fn rejected_vectors_make_the_reference_decoder_raise() {
    for vector in &codec_vectors() {
        let VectorExpectation::Rejected(marker) = &vector.expectation else {
            continue;
        };
        let error = decode_reference(&vector.input)
            .err()
            .unwrap_or_else(|| panic!("vector {} must be rejected", vector.name));
        assert_eq!(
            &error, marker,
            "vector {} rejects with its class marker",
            vector.name
        );
    }
}
