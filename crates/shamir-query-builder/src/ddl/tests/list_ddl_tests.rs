//! Tests for List ops and list functions / validators / folders.

use shamir_types::mpack;

use crate::ddl;

use super::helpers::roundtrip;

// ============================================================================
// List operations
// ============================================================================

#[test]
fn list_databases_wire() {
    let op = ddl::list_databases();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "databases"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn list_repos_wire() {
    let op = ddl::list_repos();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "repos"
        })
    );
}

#[test]
fn list_tables_wire() {
    let op = ddl::list_tables().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "tables",
            "repo": "main"
        })
    );
}

#[test]
fn list_tables_custom_repo() {
    let op = ddl::list_tables().repo("hot").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "tables",
            "repo": "hot"
        })
    );
}

#[test]
fn list_indexes_wire() {
    let op = ddl::list_indexes("users").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "indexes",
            "table": "users",
            "repo": "main"
        })
    );
}

#[test]
fn list_users_wire() {
    let op = ddl::list_users();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "users"
        })
    );
}

// ============================================================================
// list functions / validators / function_folders
// ============================================================================

#[test]
fn list_functions_wire() {
    let op = ddl::list_functions().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "functions"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn list_functions_with_folder_wire() {
    let op = ddl::list_functions().folder("math").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "functions",
            "folder": "math"
        })
    );
}

#[test]
fn list_all_validators_wire() {
    let op = ddl::list_all_validators();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "validators"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn list_function_folders_wire() {
    let op = ddl::list_function_folders().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "function_folders"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn list_function_folders_with_parent_wire() {
    let op = ddl::list_function_folders().parent("alpha").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list": "function_folders",
            "parent": "alpha"
        })
    );
}
