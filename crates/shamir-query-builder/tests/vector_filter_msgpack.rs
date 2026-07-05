//! Cross-language msgpack fixture + wire round-trip for `Filter::VectorSimilarity`
//! with the V1.1 additive `ef_search` / `oversample` fields.
//!
//! Rust half of the TS-parity contract. Pins three invariants:
//!
//! * **Part A — fixture regression:** rebuilding each canonical shape through
//!   the `shamir-query-builder` filter builders and serializing with
//!   `rmp_serde::to_vec_named` reproduces the exact hex bytes captured in
//!   [`fixtures/vector_filter_msgpack.json`]. Any drift in the Rust wire shape
//!   (renamed field, reordered struct, changed `skip_serializing_if`) fails.
//!
//! * **Part B — wire round-trip:** the bytes the builder produces decode back
//!   to the *same* `Filter` variant through msgpack.
//!
//! * **Part C — back-compat:** a hand-built msgpack map that OMITS
//!   `ef_search`/`oversample` (a pre-V1.1 client payload) deserializes with
//!   both fields = `None`. Old clients never break.
//!
//! `rmp_serde::to_vec_named` writes struct fields in DECLARATION order — the
//! fixture's `_key_order_note` documents this for the TS client. The `Filter`
//! enum is `#[serde(tag = "op", rename_all = "snake_case")]`, so the
//! discriminator key `"vector_similarity"` is the map's first entry.

use shamir_query_builder::filter;
use shamir_query_types::filter::Filter;
use std::collections::BTreeMap;

/// The canonical `VectorSimilarity` shapes — one per additive-field
/// combination, rebuilt through the public filter builders.
fn canonical_filters() -> Vec<(&'static str, Filter)> {
    vec![
        // (a) Bare: no ef_search, no oversample → pre-V1.1 wire shape.
        (
            "vector_similarity_bare",
            filter::vector_similarity("emb", vec![1.0_f32, 0.0, 0.5], 10),
        ),
        // (b) ef_search only.
        (
            "vector_similarity_ef_search",
            filter::vector_similarity_ef("emb", vec![1.0_f32, 0.0, 0.5], 10, 400),
        ),
        // (c) Both ef_search and oversample.
        (
            "vector_similarity_ef_and_oversample",
            filter::vector_similarity_opts(
                "emb",
                vec![1.0_f32, 0.0, 0.5],
                10,
                Some(400),
                Some(2.0),
            ),
        ),
    ]
}

/// Load the fixture, returning only the label → hex entries (ignoring the
/// `_`-prefixed documentation keys, which are JSON objects/strings).
fn load_fixture() -> BTreeMap<String, String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/vector_filter_msgpack.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"));
    let raw: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture JSON: {e}"));
    let obj = raw.as_object().expect("fixture root must be a JSON object");
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if k.starts_with('_') {
            continue;
        }
        let hex = v
            .as_str()
            .unwrap_or_else(|| panic!("fixture entry `{k}` must be a hex string, got {v}"));
        out.insert(k.clone(), hex.to_string());
    }
    out
}

// ============================================================================
// Part A — fixture regression: builder output matches the captured hex.
// ============================================================================

/// Helper: serialize a filter and print the hex (used to (re)generate the
/// fixture JSON — run with `--nocapture` to see the hex on stdout).
#[test]
fn generate_fixture_hex() {
    for (label, f) in canonical_filters() {
        let bytes =
            rmp_serde::to_vec_named(&f).unwrap_or_else(|e| panic!("serialize {label}: {e}"));
        let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        println!("GEN  \"{label}\": \"{hex}\",");
    }
}

#[test]
fn fixture_has_exactly_three_shapes() {
    let fx = load_fixture();
    let canonical: Vec<&str> = canonical_filters().iter().map(|(k, _)| *k).collect();
    let fixture_keys: Vec<&str> = fx.keys().map(|s| s.as_str()).collect();
    assert_eq!(
        fixture_keys.len(),
        3,
        "fixture must pin exactly 3 shapes; got {fixture_keys:?}"
    );
    for label in &canonical {
        assert!(
            fx.contains_key(*label),
            "fixture missing `{label}`; have {:?}",
            fx.keys().collect::<Vec<_>>()
        );
    }
}

#[test]
fn builder_reproduces_fixture_hex() {
    let fx = load_fixture();
    for (label, f) in canonical_filters() {
        let bytes =
            rmp_serde::to_vec_named(&f).unwrap_or_else(|e| panic!("serialize {label}: {e}"));
        let actual_hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        let expected_hex = fx
            .get(label)
            .unwrap_or_else(|| panic!("fixture has no entry for `{label}`"));
        assert_eq!(
            actual_hex, *expected_hex,
            "wire-shape drift for `{label}`: builder produced {actual_hex}, fixture expects {expected_hex}. \
             Check for renamed fields, reordered struct fields, or a changed skip_serializing_if."
        );
    }
}

// ============================================================================
// Part B — wire round-trip: builder bytes → Filter (direct decode).
// ============================================================================

#[test]
fn builder_output_round_trips() {
    for (label, f) in canonical_filters() {
        let bytes =
            rmp_serde::to_vec_named(&f).unwrap_or_else(|e| panic!("serialize {label}: {e}"));
        let decoded: Filter =
            rmp_serde::from_slice(&bytes).unwrap_or_else(|e| panic!("decode {label}: {e}"));
        assert_eq!(
            decoded, f,
            "wire round-trip mismatch for `{label}`: builder bytes did not decode back to the same Filter"
        );
    }
}

// ============================================================================
// Part C — back-compat: a pre-V1.1 payload (no ef_search/oversample) decodes.
// ============================================================================

/// A hand-built msgpack map with ONLY `op`/`field`/`query`/`k` — the exact
/// shape a pre-V1.1 client emits. Must decode with ef_search = oversample = None.
#[test]
fn pre_v11_payload_without_ef_fields_decodes_with_none() {
    // Serialize the bare builder form (no ef fields) and strip any trailing
    // ef_search/oversample keys — here the bare builder already OMITS them
    // (skip_serializing_if = Option::is_none), so the bytes are already a
    // valid pre-V1.1 payload.
    let bare = filter::vector_similarity("v", vec![0.0_f32], 1);
    let bytes = rmp_serde::to_vec_named(&bare).unwrap();
    let decoded: Filter = rmp_serde::from_slice(&bytes).unwrap();
    match decoded {
        Filter::VectorSimilarity {
            ef_search,
            oversample,
            ..
        } => {
            assert_eq!(ef_search, None);
            assert_eq!(oversample, None);
        }
        other => panic!("expected VectorSimilarity, got {other:?}"),
    }
}

/// `ef_search = u32::MAX` survives deserialization (server-side clamp is in
/// the adapter, NOT in serde — the wire layer accepts any u32).
#[test]
fn ef_search_u32_max_round_trips() {
    let f = filter::vector_similarity_ef("v", vec![0.0_f32], 1, u32::MAX);
    let bytes = rmp_serde::to_vec_named(&f).unwrap();
    let decoded: Filter = rmp_serde::from_slice(&bytes).unwrap();
    match decoded {
        Filter::VectorSimilarity { ef_search, .. } => {
            assert_eq!(ef_search, Some(u32::MAX));
        }
        other => panic!("expected VectorSimilarity, got {other:?}"),
    }
}

// ============================================================================
// Part D — TS-style payload (integer-valued floats as positive-fixint) decodes.
// ============================================================================

/// `@msgpack/msgpack` encodes integer-valued JS floats (1.0, 0.0, 2.0) as
/// msgpack positive-fixint, NOT float32. Rust `rmp_serde::from_slice` must
/// decode these into `Vec<f32>` / `Option<f32>` without error — this is the
/// TS→Rust mutual-decodability half of the parity contract (byte-identity is
/// impossible here, see the fixture's `_parity_note`).
///
/// We pin the exact bytes the TS encoder emits as literals rather than
/// re-deriving them through an encoder — the whole point is that these are the
/// foreign (TS) bytes, and a literal is both clearer and dependency-free.
#[test]
fn ts_style_int_float_payload_decodes_in_rust() {
    // The query array [1.0, 0.0, 0.5] exactly as `@msgpack/msgpack` emits it:
    //   0x93            fixarray, len 3
    //   0x01            positive fixint 1     (TS: integer-valued 1.0)
    //   0x00            positive fixint 0     (TS: integer-valued 0.0)
    //   0xca 3f000000   float32 0.5           (TS: non-integer float)
    let ts_query: [u8; 8] = [0x93, 0x01, 0x00, 0xca, 0x3f, 0x00, 0x00, 0x00];
    let decoded_query: Vec<f32> = rmp_serde::from_slice(&ts_query)
        .expect("TS-style mixed int/float32 query array must decode to Vec<f32>");
    assert_eq!(decoded_query, vec![1.0_f32, 0.0, 0.5]);

    // oversample = 2.0 in TS → positive fixint 0x02 → Option<f32> = Some(2.0).
    let ts_oversample: [u8; 1] = [0x02];
    let decoded_oversample: Option<f32> = rmp_serde::from_slice(&ts_oversample)
        .expect("TS-style integer oversample must decode to Option<f32>");
    assert_eq!(decoded_oversample, Some(2.0));
}
