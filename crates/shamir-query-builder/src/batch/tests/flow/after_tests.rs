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

#[test]
fn try_build_catches_after_path_tail() {
    let mut b = Batch::new();
    let _mk = b.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    let rows = b.insert(
        "rows",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    // Manually push a garbage path-tail into `after` (bypassing the fluent
    // API, mirroring how the planner-side test constructs this case).
    b.queries
        .get_mut(rows.alias())
        .unwrap()
        .after
        .push("mk_tbl[0].id".to_string());

    let err = b.try_build().unwrap_err();
    assert!(
        matches!(
            &err,
            BuildError::AfterPathIgnored { alias, raw }
                if alias == "rows" && raw == "mk_tbl[0].id"
        ),
        "expected AfterPathIgnored, got: {:?}",
        err
    );
}

#[test]
fn query_after_registers_dependency_equivalent_to_post_hoc_after() {
    use crate::query::Query;

    let mut b = Batch::new();
    let mk = b.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    let rows = b.query_after("rows", Query::from("users"), &[&mk]);

    let req = b.build();
    let rows_entry = req.queries.get(rows.alias()).unwrap();
    assert_eq!(rows_entry.after, vec!["mk_tbl".to_string()]);
}

#[test]
fn insert_after_registers_dependency_equivalent_to_post_hoc_after() {
    let mut b = Batch::new();
    let mk = b.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    let rows = b.insert_after(
        "rows",
        write::insert("users").row(doc().set("name", "Alice")),
        &[&mk],
    );

    let via_fluent = b.build().queries.get(rows.alias()).unwrap().after.clone();

    // Build an equivalent batch using the post-hoc `after()` and compare.
    let mut b2 = Batch::new();
    let mk2 = b2.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    let rows2 = b2.insert(
        "rows",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b2.after(&rows2, &mk2);
    let via_post_hoc = b2.build().queries.get(rows2.alias()).unwrap().after.clone();

    assert_eq!(via_fluent, via_post_hoc);
    assert_eq!(via_fluent, vec!["mk_tbl".to_string()]);
}

// -------------------------------------------------------------------------
// Naming collision regression guard (task #630 — Epic01/C gap #7)
// -------------------------------------------------------------------------
//
// `Query::after` (keyset/seek pagination cursor, on `crate::query::Query`)
// and `Batch::after` (dependency ordering, on `crate::batch::Batch`) share a
// name but live on unrelated types with unrelated signatures. This is not a
// bug today — Rust resolves each `.after(..)` call against its receiver's
// inherent impl — but a future refactor that merges/re-exports these types
// under a shared trait, or that moves one `after` onto a blanket impl,
// could silently create an ambiguity or a wrong-method dispatch. This test
// exercises BOTH `after` methods in the same file/scope, on both types, to
// pin that today they coexist without a single compile-time or run-time
// conflict.
#[test]
fn query_after_and_batch_after_coexist_without_naming_conflict() {
    use crate::query::Query;
    use shamir_types::types::value::QueryValue;

    // `Query::after` — keyset pagination cursor.
    let q = Query::from("users")
        .order_by_asc("score")
        .after(vec![QueryValue::Int(30)], Some(2));
    let read_query = q.build();
    assert!(
        matches!(
            read_query.pagination,
            shamir_query_types::read::Pagination::After { .. }
        ),
        "Query::after must build keyset pagination, got {:?}",
        read_query.pagination
    );

    // `Batch::after` — explicit dependency ordering.
    let mut b = Batch::new();
    let mk = b.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    let rows = b.insert(
        "rows",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.after(&rows, &mk);
    let req = b.build();
    assert_eq!(
        req.queries.get("rows").unwrap().after,
        vec!["mk_tbl".to_string()],
        "Batch::after must register dependency ordering, unaffected by \
         Query::after existing in the same scope"
    );
}
