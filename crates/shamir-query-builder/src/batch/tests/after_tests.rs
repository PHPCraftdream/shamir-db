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
fn after_wire_contains_after_for_dependent_only() {
    use crate::wire::ToWire;
    use shamir_types::types::value::QueryValue;

    let mut b = Batch::new();
    b.id(1);
    let mk = b.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    let rows = b.insert(
        "rows",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.after(&rows, &mk);

    let qv = b.build().to_query_value().unwrap();
    let queries = qv.get("queries").expect("queries key");

    // "rows" must have `after: ["mk_tbl"]`
    let rows_entry = queries.get("rows").expect("rows entry");
    let after = rows_entry
        .get("after")
        .expect("after key on dependent entry");
    let list = match after {
        QueryValue::List(l) => l,
        other => panic!("expected List for 'after', got {other:?}"),
    };
    assert_eq!(list.len(), 1, "dependent entry must have exactly one after");
    assert_eq!(
        list[0].as_str(),
        Some("mk_tbl"),
        "dependent entry must have 'after' pointing to mk_tbl"
    );

    // "mk_tbl" must NOT have an `after` key (empty -> omitted).
    let mk_entry = queries.get("mk_tbl").expect("mk_tbl entry");
    assert!(
        mk_entry.get("after").is_none(),
        "independent entry must NOT have 'after' in wire encoding"
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
