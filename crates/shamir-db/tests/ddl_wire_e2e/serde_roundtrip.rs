//! Serde round-trip tests for BatchOp and ListOp variants.

// ═══════════════════════════════════════════════════════════════════════
// 10. BatchOp serde round-trip for new variants
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn create_function_serde_roundtrip() {
    let json_str = r#"{"create_function": "my_fn", "wasm": "AAAA", "replace": true}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    assert!(op.table_ref().is_none());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn create_validator_serde_roundtrip() {
    let json_str = r#"{"create_validator": "v_age", "wasm": "BBBB", "replace": false}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn bind_validator_serde_roundtrip() {
    let json_str = r#"{
        "bind_validator": "v_age",
        "db": "testdb",
        "repo": "main",
        "table": "users",
        "ops": ["insert", "update"],
        "priority": 1500
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn create_function_folder_serde_roundtrip() {
    let json_str = r#"{"create_function_folder": ["reports", "daily"]}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn drop_function_serde_roundtrip() {
    let json_str = r#"{"drop_function": "my_fn"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn rename_validator_serde_roundtrip() {
    let json_str = r#"{"rename_validator": "v_old", "to": "v_new"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

// =====================================================================
// Phase 1b: serde round-trip — if_not_exists / cascade
// =====================================================================

#[test]
fn serde_create_table_if_not_exists_round_trip() {
    // With flag set
    let json_with = r#"{
        "create_table": "orders",
        "repo": "main",
        "if_not_exists": true
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_with).unwrap();
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
    assert!(
        back.contains("if_not_exists"),
        "serialised form should contain if_not_exists when true"
    );

    // With flag absent (default false) — should NOT appear in JSON
    let json_without = r#"{
        "create_table": "orders",
        "repo": "main"
    }"#;
    let op3: shamir_db::query::batch::BatchOp = serde_json::from_str(json_without).unwrap();
    let back3 = serde_json::to_string(&op3).unwrap();
    assert!(
        !back3.contains("if_not_exists"),
        "serialised form should omit if_not_exists when false, got: {back3}"
    );
}

#[test]
fn serde_drop_db_cascade_round_trip() {
    // With cascade
    let json_with = r#"{
        "drop_db": "testdb",
        "cascade": true
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_with).unwrap();
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
    assert!(
        back.contains("cascade"),
        "serialised form should contain cascade when true"
    );

    // Without cascade — should NOT appear in JSON
    let json_without = r#"{
        "drop_db": "testdb"
    }"#;
    let op3: shamir_db::query::batch::BatchOp = serde_json::from_str(json_without).unwrap();
    let back3 = serde_json::to_string(&op3).unwrap();
    assert!(
        !back3.contains("cascade"),
        "serialised form should omit cascade when false, got: {back3}"
    );
}

#[test]
fn serde_drop_repo_cascade_round_trip() {
    let json_str = r#"{
        "drop_repo": "archive",
        "cascade": true
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
    assert!(back.contains("cascade"));
}

#[test]
fn serde_create_db_if_not_exists_round_trip() {
    let json_str = r#"{
        "create_db": "mydb",
        "if_not_exists": true
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    let back = serde_json::to_string(&op).unwrap();
    assert!(back.contains("if_not_exists"));
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

// =====================================================================
// Serde round-trip for new ListOp variants
// =====================================================================

#[test]
fn serde_list_functions_round_trip() {
    let json_str = r#"{"list": "functions"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn serde_list_functions_with_folder_round_trip() {
    let json_str = r#"{"list": "functions", "folder": "math"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    assert!(
        back.contains("math"),
        "serialised form should contain folder"
    );
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn serde_list_validators_round_trip() {
    let json_str = r#"{"list": "validators"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn serde_list_function_folders_round_trip() {
    let json_str = r#"{"list": "function_folders"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn serde_list_function_folders_with_parent_round_trip() {
    let json_str = r#"{"list": "function_folders", "parent": "reports"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    assert!(
        back.contains("reports"),
        "serialised form should contain parent"
    );
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}
