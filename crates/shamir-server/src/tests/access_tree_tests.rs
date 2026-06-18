use serde_json::json;

use crate::access_tree::{mode_str, render};
use shamir_types::types::value::QueryValue;

#[test]
fn mode_str_renders_posix() {
    assert_eq!(mode_str(0o777), "rwxrwxrwx");
    assert_eq!(mode_str(0o750), "rwxr-x---");
    assert_eq!(mode_str(0o700), "rwx------");
    assert_eq!(mode_str(0o000), "---------");
    // setuid folds into owner-exec: with x → 's', without → 'S'.
    assert_eq!(mode_str(0o4755), "rwsr-xr-x");
    assert_eq!(mode_str(0o4655), "rwSr-xr-x");
}

#[test]
fn render_draws_hierarchy_and_resolves_names() {
    // Built in layers to keep the `json!` macro nesting shallow.
    let table = json!({
        "name": "users", "kind": "table",
        "owner": 42, "owner_name": "alice",
        "group": 3, "group_name": "devs",
        "mode": 0o750, "setuid": false, "children": []
    });
    let store = json!({
        "name": "main", "kind": "store", "owner": 0, "owner_name": "system",
        "group": null, "group_name": null, "mode": 0o775, "setuid": false,
        "children": [table]
    });
    let db = json!({
        "name": "mydb", "kind": "database", "owner": 0, "owner_name": "system",
        "group": null, "group_name": null, "mode": 0o775, "setuid": false,
        "children": [store]
    });
    let resources = json!({
        "name": "/", "kind": "root", "owner": 0, "owner_name": "system",
        "group": null, "group_name": null, "mode": 0o777, "setuid": false,
        "children": [db]
    });
    let functions = json!([
        {"name": "argon2id", "owner": 0, "owner_name": "system",
         "group": null, "group_name": null, "mode": 0o777, "setuid": false, "builtin": true}
    ]);
    let principals = json!({
        "users": [{"id": 42, "name": "alice"}],
        "groups": [{"id": 3, "name": "devs", "members": [{"id": 42, "name": "alice"}]}]
    });
    let tree_json = json!({
        "resources": resources,
        "functions": functions,
        "principals": principals,
    });
    // serde round-trip test: the subject is the wire format, so we convert
    // serde_json::Value → QueryValue to exercise render with live types.
    let tree = QueryValue::from(tree_json);

    let out = render(&tree);
    // Hierarchy + labels.
    assert!(out.contains("db mydb"));
    assert!(out.contains("store main"));
    assert!(out.contains("table users"));
    // Resolved owner:group + mode on the table row.
    assert!(out.contains("alice:devs"));
    assert!(out.contains("rwxr-x---"));
    // Functions section with builtin marker.
    assert!(out.contains("argon2id (builtin)"));
    // Principals section with resolved membership.
    assert!(out.contains("alice(42)"));
    assert!(out.contains("devs(3)=[alice]"));
}
