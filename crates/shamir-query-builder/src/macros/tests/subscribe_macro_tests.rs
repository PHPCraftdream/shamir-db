//! Tests for the `subscribe!` macro.

use shamir_query_types::subscribe::{EventMask, SubscribeOp};
use shamir_query_types::TableRef;

use crate::batch::subscribe::{SourceBuilder, Subscribe};
use crate::filter as filter_mod;

// ── helpers ────────────────────────────────────────────────────────

fn assert_eq_wire(a: &SubscribeOp, b: &SubscribeOp) {
    let ja = serde_json::to_value(a).unwrap();
    let jb = serde_json::to_value(b).unwrap();
    assert_eq!(ja, jb, "wire JSON mismatch:\n  left:  {ja}\n  right: {jb}");
}

// ── tests ──────────────────────────────────────────────────────────

#[test]
fn basic_source_and_filter() {
    let from_macro = subscribe! {
        source: ("main", "messages"),
        where: filter_mod::eq("thread_id", 42),
    };

    let expected = Subscribe::source(
        SourceBuilder::table(TableRef::with_repo("main", "messages"))
            .filter(filter_mod::eq("thread_id", 42))
            .build(),
    )
    .build();

    assert_eq_wire(&from_macro, &expected);
}

#[test]
fn with_event_put() {
    let from_macro = subscribe! {
        source: ("main", "messages"),
        where: filter_mod::eq("status", "active"),
        on: put,
    };

    let expected = Subscribe::source(
        SourceBuilder::table(TableRef::with_repo("main", "messages"))
            .filter(filter_mod::eq("status", "active"))
            .events(EventMask::Put)
            .build(),
    )
    .build();

    assert_eq_wire(&from_macro, &expected);
}

#[test]
fn with_event_delete() {
    let from_macro = subscribe! {
        source: ("main", "messages"),
        where: filter_mod::eq("status", "active"),
        on: delete,
    };

    let expected = Subscribe::source(
        SourceBuilder::table(TableRef::with_repo("main", "messages"))
            .filter(filter_mod::eq("status", "active"))
            .events(EventMask::Delete)
            .build(),
    )
    .build();

    assert_eq_wire(&from_macro, &expected);
}

#[test]
fn with_initial() {
    let from_macro = subscribe! {
        source: ("main", "users"),
        where: filter_mod::eq("online", true),
        initial: true,
    };

    let expected = Subscribe::source(
        SourceBuilder::table(TableRef::with_repo("main", "users"))
            .filter(filter_mod::eq("online", true))
            .build(),
    )
    .with_initial()
    .build();

    assert_eq_wire(&from_macro, &expected);
}

#[test]
fn deliver_keys() {
    let from_macro = subscribe! {
        source: ("main", "users"),
        where: filter_mod::eq("online", true),
        deliver: keys,
    };

    let expected = Subscribe::source(
        SourceBuilder::table(TableRef::with_repo("main", "users"))
            .filter(filter_mod::eq("online", true))
            .build(),
    )
    .deliver_keys()
    .build();

    assert_eq_wire(&from_macro, &expected);
}

#[test]
fn full_form() {
    let from_macro = subscribe! {
        source: ("main", "messages"),
        where: filter_mod::eq("thread_id", 42),
        on: put,
        initial: true,
        from_version: 100,
        deliver: keys,
    };

    let expected = Subscribe::source(
        SourceBuilder::table(TableRef::with_repo("main", "messages"))
            .filter(filter_mod::eq("thread_id", 42))
            .events(EventMask::Put)
            .build(),
    )
    .deliver_keys()
    .with_initial()
    .from_version(100)
    .build();

    assert_eq_wire(&from_macro, &expected);
}
