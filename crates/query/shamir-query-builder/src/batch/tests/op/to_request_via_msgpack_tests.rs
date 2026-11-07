use shamir_types::mpack;

use crate::batch::{Batch, BuildError};
use crate::query::Query;

#[test]
fn borrowed_msgpack_avoids_payload_clone_allocations() {
    let payload = vec![0x5au8; 256 * 1024];
    let mut batch = Batch::new();
    batch.insert(
        "large",
        crate::write::Insert::into("items")
            .row(mpack!({ "payload": @ shamir_types::types::value::QueryValue::Bin(payload) })),
    );

    let (borrowed_bytes, borrowed) =
        crate::testing_alloc::count_allocated_bytes(|| batch.to_msgpack().unwrap());
    let (owned_bytes, owned) = crate::testing_alloc::count_allocated_bytes(|| {
        rmp_serde::to_vec_named(&batch.try_build().unwrap()).unwrap()
    });
    assert_eq!(borrowed, owned);
    assert!(
        owned_bytes >= borrowed_bytes.saturating_add(256 * 1024),
        "owned encoding allocated {owned_bytes} bytes, borrowed encoding {borrowed_bytes}"
    );
}

#[test]
fn to_msgpack_borrows_request_and_matches_owned_request_encoding() {
    let mut batch = Batch::named("rich");
    batch.id(shamir_types::types::value::QueryValue::Int(99));
    batch.transactional();
    batch.isolation(crate::batch::Isolation::Serializable);
    batch.durability(crate::batch::Durability::Synced);
    batch.index_visibility(crate::batch::IndexVisibility::Async);
    batch.visibility_wait(shamir_query_types::batch::VisibilityWait::Bounded { timeout_ms: 25 });
    batch.query("q", Query::from("items"));
    let expected = batch.try_build().unwrap();
    let borrowed = batch.to_msgpack().unwrap();
    let owned = rmp_serde::to_vec_named(&expected).unwrap();
    assert_eq!(borrowed, owned);
}

#[test]
fn rv_query_builder_ee1751fb_returns_build_error_instead_of_panicking() {
    let mut batch = Batch::new();
    batch.query("duplicate", Query::from("first"));
    batch.query("duplicate", Query::from("second"));

    assert!(matches!(
        batch.to_request_via_msgpack(),
        Err(BuildError::DuplicateAlias { alias }) if alias == "duplicate"
    ));
}

/// The round-tripped `BatchRequest` must equal the one from `build()`.
#[test]
fn round_trip_matches_build() {
    let mut b = Batch::new();
    b.query("q1", Query::from("users"));
    b.insert(
        "ins",
        crate::write::Insert::into("users").row(mpack!({ "name": "alice" })),
    );

    let via_build = b.build();
    let via_msgpack = b.to_request_via_msgpack().unwrap();
    assert_eq!(via_build, via_msgpack);
}

/// Named batch with config options survives the round-trip.
#[test]
fn named_batch_with_options_survives_round_trip() {
    let mut b = Batch::named("test-batch");
    b.transactional();
    b.isolation(crate::batch::Isolation::Serializable);
    b.durability(crate::batch::Durability::Synced);
    b.query("q", Query::from("items"));

    let via_build = b.build();
    let via_msgpack = b.to_request_via_msgpack().unwrap();
    assert_eq!(via_build, via_msgpack);
}

/// Empty batch (no entries) round-trips correctly.
#[test]
fn empty_batch_round_trips() {
    let b = Batch::new();

    let via_build = b.build();
    let via_msgpack = b.to_request_via_msgpack().unwrap();
    assert_eq!(via_build, via_msgpack);
}
