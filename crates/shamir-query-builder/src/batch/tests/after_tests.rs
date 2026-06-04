//! Tests for the `Batch::after` explicit ordering API.

use crate::batch::{Batch, BuildError};
use crate::ddl;
use crate::write::{self, doc};

#[test]
fn after_sets_field_on_dependent_entry() {
    let mut b = Batch::new();
    let mk = b.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    let rows = b.insert(
        "rows",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.after(&rows, &mk);

    let req = b.build();
    let rows_entry = req.queries.get("rows").unwrap();
    assert_eq!(rows_entry.after, vec!["mk_tbl".to_string()]);
}

#[test]
fn after_wire_json_contains_after_for_dependent_only() {
    let mut b = Batch::new();
    b.id(1);
    let mk = b.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    let rows = b.insert(
        "rows",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.after(&rows, &mk);

    let json = b.to_json_value().unwrap();
    let queries = json["queries"].as_object().unwrap();

    // "rows" must have `"after": ["mk_tbl"]`
    let rows_json = &queries["rows"];
    assert_eq!(
        rows_json["after"],
        serde_json::json!(["mk_tbl"]),
        "dependent entry must have 'after' in wire JSON"
    );

    // "mk_tbl" must NOT have `after` key at all (empty -> omitted).
    let mk_json = &queries["mk_tbl"];
    assert!(
        mk_json.get("after").is_none(),
        "independent entry must NOT have 'after' in wire JSON"
    );
}

#[test]
fn try_build_catches_unknown_after_alias() {
    let mut b = Batch::new();
    let _mk = b.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    // Manually push an unknown alias into after.
    b.queries
        .get_mut("mk_tbl")
        .unwrap()
        .after
        .push("nonexistent".to_string());

    let err = b.try_build().unwrap_err();
    assert!(
        matches!(
            &err,
            BuildError::UnknownAlias {
                alias,
                referenced_by,
            } if alias == "nonexistent" && referenced_by == "mk_tbl"
        ),
        "expected UnknownAlias, got: {:?}",
        err
    );
}

#[test]
fn try_build_catches_self_after() {
    let mut b = Batch::new();
    let _mk = b.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    // Push self-reference into after.
    b.queries
        .get_mut("mk_tbl")
        .unwrap()
        .after
        .push("mk_tbl".to_string());

    let err = b.try_build().unwrap_err();
    assert!(
        matches!(
            &err,
            BuildError::SelfReference { alias } if alias == "mk_tbl"
        ),
        "expected SelfReference, got: {:?}",
        err
    );
}
