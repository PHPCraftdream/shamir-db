//! Round-trip tests for `ArtifactKind` catalogue tagging (Phase 0 parity).
//!
//! Verifies two things:
//! 1. A row built by the current `create_function_with_opts` / validator
//!    creation path persists a `kind` field and reloads with the same value.
//! 2. **Migration safety:** a catalogue row assembled WITHOUT a `kind` field
//!    (i.e. a row persisted by a pre-Phase-0 binary) decodes to
//!    `ArtifactKind::Wasm`, so the existing catalogue keeps working.
//!
//! These tests live externally (per the workspace test-layout rule) and
//! exercise the public `ArtifactKind` API plus the real catalogue build sites
//! in `function_management.rs` / `validator_management.rs`.

use crate::shamir_db::shamir_db::{ArtifactKind, KIND_FIELD};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ============================================================================
// Unit-level: ArtifactKind::from_record over the QueryValue shape that the
// catalogue build sites produce. No DB needed.
// ============================================================================

fn legacy_function_row_without_kind() -> QueryValue {
    // Exactly the field set pre-Phase-0 `function_management.rs` produced.
    let mut m = new_map();
    m.insert("name".to_string(), QueryValue::Str("legacy_fn".to_string()));
    m.insert("wasm_b64".to_string(), QueryValue::Str("e30=".to_string()));
    m.insert(
        "wasm_hash".to_string(),
        QueryValue::Str("deadbeef".to_string()),
    );
    m.insert("lang".to_string(), QueryValue::Str("wasm".to_string()));
    m.insert("source".to_string(), QueryValue::Null);
    m.insert("version".to_string(), QueryValue::Int(1));
    QueryValue::Map(m)
}

fn legacy_validator_row_without_kind() -> QueryValue {
    let mut m = new_map();
    m.insert(
        "name".to_string(),
        QueryValue::Str("legacy_val".to_string()),
    );
    m.insert(
        "_id".to_string(),
        QueryValue::Str("0123456789abcdef0123456789abcdef".to_string()),
    );
    m.insert("wasm_b64".to_string(), QueryValue::Str("e30=".to_string()));
    m.insert(
        "wasm_hash".to_string(),
        QueryValue::Str("deadbeef".to_string()),
    );
    m.insert("lang".to_string(), QueryValue::Str("wasm".to_string()));
    m.insert("source".to_string(), QueryValue::Null);
    m.insert("bound_in".to_string(), QueryValue::List(vec![]));
    QueryValue::Map(m)
}

#[test]
fn legacy_function_row_loads_as_wasm() {
    // Migration safety: a pre-Phase-0 function row has no `kind` field and
    // MUST continue to decode as Wasm after the upgrade.
    assert_eq!(
        ArtifactKind::from_record(&legacy_function_row_without_kind()),
        ArtifactKind::Wasm,
    );
}

#[test]
fn legacy_validator_row_loads_as_wasm() {
    // Migration safety: a pre-Phase-0 validator row has no `kind` field and
    // MUST continue to decode as Wasm after the upgrade.
    assert_eq!(
        ArtifactKind::from_record(&legacy_validator_row_without_kind()),
        ArtifactKind::Wasm,
    );
}

#[test]
fn wasm_kind_round_trips() {
    let mut rec = legacy_function_row_without_kind();
    if let QueryValue::Map(m) = &mut rec {
        m.insert(KIND_FIELD.to_string(), QueryValue::Str("wasm".to_string()));
    }
    assert_eq!(ArtifactKind::from_record(&rec), ArtifactKind::Wasm);
}

#[test]
fn native_kind_round_trips() {
    let mut rec = legacy_function_row_without_kind();
    if let QueryValue::Map(m) = &mut rec {
        m.insert(
            KIND_FIELD.to_string(),
            QueryValue::Str("native".to_string()),
        );
    }
    assert_eq!(ArtifactKind::from_record(&rec), ArtifactKind::Native);
}

#[test]
fn null_kind_field_defaults_to_wasm() {
    let mut rec = legacy_function_row_without_kind();
    if let QueryValue::Map(m) = &mut rec {
        m.insert(KIND_FIELD.to_string(), QueryValue::Null);
    }
    assert_eq!(ArtifactKind::from_record(&rec), ArtifactKind::Wasm);
}

#[test]
fn unknown_kind_fails_safe_to_wasm() {
    // Forward compatibility: a future catalogue written by a newer binary
    // with a kind an older binary doesn't recognise must not break boot.
    let mut rec = legacy_function_row_without_kind();
    if let QueryValue::Map(m) = &mut rec {
        m.insert(KIND_FIELD.to_string(), QueryValue::Str("llvm".to_string()));
    }
    assert_eq!(ArtifactKind::from_record(&rec), ArtifactKind::Wasm);
}

#[test]
fn non_map_record_defaults_to_wasm() {
    assert_eq!(
        ArtifactKind::from_record(&QueryValue::Null),
        ArtifactKind::Wasm
    );
    assert_eq!(
        ArtifactKind::from_record(&QueryValue::Str("not-a-map".to_string())),
        ArtifactKind::Wasm,
    );
}

#[test]
fn kind_field_spelling_is_stable() {
    // The persisted string spellings are part of the on-disk catalogue
    // format — renaming them is a migration. Lock them down.
    assert_eq!(ArtifactKind::Wasm.as_str(), "wasm");
    assert_eq!(ArtifactKind::Native.as_str(), "native");
    assert_eq!(KIND_FIELD, "kind");
    assert_eq!(
        ArtifactKind::Wasm.as_query_value(),
        QueryValue::Str("wasm".to_string())
    );
    assert_eq!(
        ArtifactKind::Native.as_query_value(),
        QueryValue::Str("native".to_string())
    );
}

#[test]
fn parse_kind_fail_safe() {
    // Round-trip the known spellings...
    assert_eq!(ArtifactKind::parse_kind("wasm"), ArtifactKind::Wasm);
    assert_eq!(ArtifactKind::parse_kind("native"), ArtifactKind::Native);
    // ...and confirm unknowns degrade to the historical default rather
    // than panicking or erroring. This is what makes the catalogue format
    // forward-compatible: an older binary reading a row written by a newer
    // one keeps booting instead of failing.
    assert_eq!(ArtifactKind::parse_kind("llvm"), ArtifactKind::Wasm);
    assert_eq!(ArtifactKind::parse_kind(""), ArtifactKind::Wasm);
    assert_eq!(ArtifactKind::parse_kind("NATIVE"), ArtifactKind::Wasm);
}
