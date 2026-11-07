//! Regression tests for deferred registration errors:
//! - duplicate alias registration keeps the FIRST op and errors on `try_build`
//! - `after`/`when` with an unregistered alias errors on `try_build`
//!   (no silent no-op).

use shamir_query_types::batch::BatchOp;

use crate::batch::{Batch, BuildError, Handle};
use crate::filter;
use crate::query::Query;
use crate::write::{self, doc};

#[test]
fn owned_request_preserves_deferred_error_precedence() {
    let mut batch = Batch::new();
    batch.query(
        "dup",
        Query::from("t").where_eq("id", crate::val::qref_all("missing")),
    );
    batch.query("dup", Query::from("other"));
    let borrowed = batch.try_build().unwrap_err();
    assert!(matches!(&borrowed, BuildError::DuplicateAlias { alias } if alias == "dup"));
    assert_eq!(batch.try_into_request().unwrap_err(), borrowed);
}

#[test]
fn rv_query_builder_63b52b5c_duplicate_alias_keeps_first_op_and_errors() {
    let mut b = Batch::new();
    b.query("dup", Query::from("first_table"));
    b.insert(
        "dup",
        write::insert("second_table").row(doc().set("a", 1).unwrap()),
    );

    // try_build must error on the duplicate alias.
    let err = b.try_build().unwrap_err();
    assert!(
        matches!(&err, BuildError::DuplicateAlias { alias } if alias == "dup"),
        "expected DuplicateAlias, got {err:?}"
    );

    // The FIRST operation must survive: still a Read of first_table.
    let req = b.build();
    let entry = req.queries.get("dup").expect("first entry kept");
    assert!(
        matches!(entry.op, BatchOp::Read(_)),
        "first op must not be replaced by the later registration, got {:?}",
        entry.op
    );
}

#[test]
fn rv_query_builder_2fd16c76_after_and_when_unknown_alias_error_in_try_build() {
    let mut b = Batch::new();
    let h = b.query("known", Query::from("t"));
    let ghost = Handle {
        alias: "ghost".into(),
    };

    b.after(&h, &ghost);
    b.when(&ghost, filter::eq("x", 1));

    // First deferred error is the `after` miss; the `when` miss must also be
    // recorded (an unfixed `after` alone would leave try_build succeeding).
    let err = b.try_build().unwrap_err();
    assert!(
        matches!(
            &err,
            BuildError::UnknownOrderingTarget {
                alias,
                operation: "after",
            } if alias == "ghost"
        ),
        "expected UnknownOrderingTarget after/ghost, got {err:?}"
    );

    // The `when` miss is recorded too.
    assert_eq!(b.deferred_errors.len(), 2);
    assert!(matches!(
        &b.deferred_errors[1],
        BuildError::UnknownOrderingTarget {
            alias,
            operation: "when",
        } if alias == "ghost"
    ));
}

// rv_query_builder_63b52b5c_to_msgpack_surfaces_duplicate_alias
#[test]
fn rv_query_builder_63b52b5c_to_msgpack_surfaces_duplicate_alias() {
    // `to_msgpack` is a production wire path — it must route through
    // `try_build` so the deferred duplicate-alias error surfaces as `Err`
    // instead of encoding a silently-truncated request.
    let mut b = Batch::new();
    b.query("dup", Query::from("first_table"));
    b.insert(
        "dup",
        write::insert("second_table").row(doc().set("a", 1).unwrap()),
    );

    let err = b.to_msgpack().unwrap_err();
    assert!(
        matches!(&err, BuildError::DuplicateAlias { alias } if alias == "dup"),
        "expected DuplicateAlias from to_msgpack, got {err:?}"
    );
}

// rv_query_builder_2fd16c76_to_msgpack_surfaces_unknown_handle
#[test]
fn rv_query_builder_2fd16c76_to_msgpack_surfaces_unknown_handle() {
    // A deferred `after` miss (unknown handle) must also fail the wire path.
    let mut b = Batch::new();
    let h = b.query("known", Query::from("t"));
    let ghost = Handle {
        alias: "ghost".into(),
    };
    b.after(&h, &ghost);

    let err = b.to_msgpack().unwrap_err();
    assert!(
        matches!(
            &err,
            BuildError::UnknownOrderingTarget {
                alias,
                operation: "after",
            } if alias == "ghost"
        ),
        "expected UnknownOrderingTarget from to_msgpack, got {err:?}"
    );
}
