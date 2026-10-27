use shamir_types::mpack;

use crate::batch::{Batch, BuildError};
use crate::query::Query;

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
