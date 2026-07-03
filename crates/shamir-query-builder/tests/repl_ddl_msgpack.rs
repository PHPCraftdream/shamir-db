//! Cross-language msgpack fixture + wire round-trip for the 10 replication-DDL
//! `BatchOp` variants.
//!
//! This is the Rust half of the TS-parity contract tracked in #376. It pins
//! two invariants:
//!
//! * **Part B — fixture regression:** rebuilding each op through the
//!   `shamir-query-builder` DDL builders and serializing with
//!   `rmp_serde::to_vec_named` reproduces the exact hex bytes captured in
//!   [`fixtures/repl_ddl_msgpack.json`]. Any drift in the Rust wire shape
//!   (a renamed field, a reordered enum variant, a changed `skip_serializing_if`)
//!   fails here.
//!
//! * **Part C — wire round-trip:** the bytes the builder produces decode back
//!   to the *same* `BatchOp` variant through the server's decode path. The
//!   server decodes a `BatchOp` by first materializing the wire map as a
//!   `QueryValue`, then dispatching on the discriminator key (see
//!   `BatchOp::deserialize` in `shamir-query-types::batch::batch_op`). We
//!   exercise that exact path here: `bytes → QueryValue → BatchOp`. This proves
//!   the builder's output is server-parseable — execution is out of scope.
//!
//! Determinism: every input string is a fixed literal; no timestamps, randomness,
//! or HashMap iteration leaks into a serialized field. `rmp_serde::to_vec_named`
//! emits maps with keys in **struct declaration order** (not alphabetical) —
//! the fixture's `_key_order_note` documents this for the TS client in #376.

use shamir_query_builder::ddl;
use shamir_query_builder::ddl::{ReplDirection, ReplMode};
use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::{QueryValue, Value};
use std::collections::BTreeMap;

/// The canonical 10 repl-DDL `BatchOp` variants — one entry per variant,
/// rebuilt through the public DDL builders. Inputs are fixed literals for
/// deterministic wire bytes. `AlterSubscription` is represented by its most
/// informative `SubAction` (`SetProfile` is the sole payload-carrying variant;
/// `Pause`/`Resume` are snake_case unit strings already pinned by the workspace
/// enum-shape test `repl_nested_enums_wire_shape`).
fn canonical_ops() -> Vec<(&'static str, BatchOp)> {
    vec![
        (
            "create_replication_profile",
            ddl::replication_profile("cluster")
                .stream(
                    ddl::repl_scope("app").repo("main").build(),
                    ReplDirection::Pull,
                    ReplMode::ReadOnly,
                )
                .build(),
        ),
        (
            "drop_replication_profile",
            ddl::drop_replication_profile("cluster"),
        ),
        (
            "create_publication",
            ddl::publication("pub_all")
                .scope(ddl::repl_scope("app").build())
                .build(),
        ),
        ("drop_publication", ddl::drop_publication("pub_all")),
        (
            "create_subscription",
            ddl::subscription("sub1", "leader:9above", "pub_all", "cluster"),
        ),
        ("drop_subscription", ddl::drop_subscription("sub1")),
        (
            "alter_subscription_set_profile",
            ddl::alter_subscription("sub1")
                .set_profile("cluster2")
                .build(),
        ),
        ("list_publications", ddl::list_publications()),
        ("list_subscriptions", ddl::list_subscriptions()),
        ("replication_status", ddl::replication_status()),
    ]
}

/// Load the fixture, returning only the 10 op-label → hex entries (ignoring the
/// `_comment` / `_key_order_note` / `_value_notes` documentation keys, which
/// are JSON objects/strings rather than hex strings).
fn load_fixture() -> BTreeMap<String, String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/repl_ddl_msgpack.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"));
    let raw: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture JSON: {e}"));
    let obj = raw.as_object().expect("fixture root must be a JSON object");
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        // Skip documentation keys (prefixed `_`) and any non-string value.
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
// Part B — fixture regression: builder output matches the captured hex.
// ============================================================================

#[test]
fn fixture_has_exactly_ten_ops() {
    let fx = load_fixture();
    let canonical: Vec<&str> = canonical_ops().iter().map(|(k, _)| *k).collect();
    let fixture_keys: Vec<&str> = fx.keys().map(|s| s.as_str()).collect();
    assert_eq!(
        fixture_keys.len(),
        10,
        "fixture must pin exactly 10 ops (one per BatchOp variant); got {fixture_keys:?}"
    );
    for label in &canonical {
        assert!(
            fx.contains_key(*label),
            "fixture missing op `{label}`; have {:?}",
            fx.keys().collect::<Vec<_>>()
        );
    }
    assert_eq!(
        canonical.len(),
        10,
        "canonical_ops must enumerate all 10 variants"
    );
}

#[test]
fn builder_reproduces_fixture_hex() {
    let fx = load_fixture();
    for (label, op) in canonical_ops() {
        let bytes =
            rmp_serde::to_vec_named(&op).unwrap_or_else(|e| panic!("serialize {label}: {e}"));
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
// Part C — wire round-trip: builder bytes → QueryValue → BatchOp (server path).
// ============================================================================

/// Exercise the exact server decode path (`bytes → QueryValue → BatchOp`) and
/// assert the decoded `BatchOp` equals the one the builder produced. This proves
/// the builder's output is server-parseable back into the same variant; op
/// *execution* is deliberately out of scope (no engine here).
#[test]
fn builder_output_round_trips_through_server_decode_path() {
    for (label, op) in canonical_ops() {
        // 1. Builder → wire bytes (the canonical contract bytes).
        let bytes =
            rmp_serde::to_vec_named(&op).unwrap_or_else(|e| panic!("serialize {label}: {e}"));

        // 2. Wire bytes → QueryValue (the intermediate map form the server
        //    buffers in `BatchOp::deserialize`).
        let qv: QueryValue = rmp_serde::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("decode {label} bytes → QueryValue: {e}"));

        // 3. QueryValue → BatchOp via the server's discriminator dispatch. The
        //    server re-encodes the QueryValue through msgpack and runs the typed
        //    Deserialize — we mirror that two-step hop exactly.
        let qv_bytes = rmp_serde::to_vec_named(&qv)
            .unwrap_or_else(|e| panic!("re-encode {label} QueryValue: {e}"));
        let decoded: BatchOp = rmp_serde::from_slice(&qv_bytes)
            .unwrap_or_else(|e| panic!("decode {label} QueryValue → BatchOp: {e}"));

        assert_eq!(
            decoded, op,
            "wire round-trip mismatch for `{label}`: the builder's bytes did not decode back to the same BatchOp variant"
        );
    }
}

/// Pin the read-only discriminator-as-`true` wire shape: the three introspection
/// ops must serialize their boolean flag as msgpack `true` (0xc3), and the
/// `false`-default is skipped (so `Op::default()` is an empty map). This is the
/// property the fixture's `list_*` / `replication_status` entries depend on.
#[test]
fn read_only_ops_emit_true_discriminator() {
    for (label, op) in canonical_ops() {
        let is_read_only = matches!(
            op,
            BatchOp::ListPublications(_)
                | BatchOp::ListSubscriptions(_)
                | BatchOp::ReplicationStatus(_)
        );
        if !is_read_only {
            continue;
        }
        let bytes = rmp_serde::to_vec_named(&op).expect("serialize");
        // The last byte for these ops is the boolean true flag (0xc3); a false
        // flag would be skipped entirely, so the entry would have zero value
        // bytes. Assert presence of 0xc3.
        assert!(
            bytes.ends_with(&[0xc3]),
            "`{label}` must end with msgpack true (0xc3); got bytes ending in {:02x?}",
            &bytes[bytes.len().saturating_sub(4)..]
        );
        // And the decoded QueryValue must be a single-key map with value `true`.
        let qv: QueryValue = rmp_serde::from_slice(&bytes).expect("qv decode");
        let map = match &qv {
            Value::Map(m) => m,
            _ => panic!("`{label}` decoded to non-map QueryValue: {qv:?}"),
        };
        assert_eq!(map.len(), 1, "`{label}` must be a single-key map");
        let (_k, v) = map.iter().next().unwrap();
        assert_eq!(
            v.as_bool(),
            Some(true),
            "`{label}` discriminator value must be true"
        );
    }
}
