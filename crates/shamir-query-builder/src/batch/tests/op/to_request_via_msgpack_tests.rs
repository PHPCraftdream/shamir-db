use shamir_types::mpack;

use crate::batch::Batch;
use crate::query::Query;

// ============================================================================
// to_request_via_msgpack round-trip equivalence
// ============================================================================

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
    let via_msgpack = b.to_request_via_msgpack();
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
    let via_msgpack = b.to_request_via_msgpack();
    assert_eq!(via_build, via_msgpack);
}

/// Empty batch (no entries) round-trips correctly.
#[test]
fn empty_batch_round_trips() {
    let b = Batch::new();

    let via_build = b.build();
    let via_msgpack = b.to_request_via_msgpack();
    assert_eq!(via_build, via_msgpack);
}
