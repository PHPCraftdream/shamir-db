//! Serde round-trip + accessor tests for the `Pagination::After` (keyset /
//! seek-pagination) variant.
//!
//! Seek semantics ("return up to `limit` rows ordered after the tuple `key`")
//! are handled by the planner in a later task; this test only covers the wire
//! DTO: serde shape, constructor, and the `keyset()` accessor.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::read::Pagination;

fn to_qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn from_qv<T: serde::de::DeserializeOwned>(qv: QueryValue) -> T {
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

/// `Pagination::After` with a `limit` round-trips with the exact wire shape
/// `{ "mode": "After", "key": [...], "limit": <n> }`.
#[test]
fn after_with_limit_round_trip() {
    let key = vec![QueryValue::Str("alice".to_string()), QueryValue::Int(42)];
    let p = Pagination::after(key.clone(), Some(10));

    let qv = to_qv(&p);
    assert_eq!(
        qv,
        mpack!({
            "mode": "After",
            "key": [ @ QueryValue::Str("alice".to_string()), @ QueryValue::Int(42) ],
            "limit": 10_i64
        })
    );

    let back: Pagination = from_qv(qv);
    assert_eq!(back, p);
}

/// `Pagination::After` without a `limit` omits the `limit` field on serialize
/// and still round-trips.
#[test]
fn after_without_limit_round_trip() {
    let key = vec![QueryValue::Int(7)];
    let p = Pagination::after(key.clone(), None);

    let qv = to_qv(&p);
    assert_eq!(
        qv,
        mpack!({
            "mode": "After",
            "key": [ @ QueryValue::Int(7) ]
        })
    );

    let back: Pagination = from_qv(qv);
    assert_eq!(back, p);
}

/// `Pagination::After` deserializes from the exact wire shape (independent of
/// the constructor).
#[test]
fn after_deserializes_from_wire_shape() {
    let qv = mpack!({
        "mode": "After",
        "key": [ @ QueryValue::Str("z".to_string()) ],
        "limit": 5_i64
    });
    let p: Pagination = from_qv(qv);
    match p {
        Pagination::After {
            key,
            limit,
            after_id,
        } => {
            assert_eq!(key, vec![QueryValue::Str("z".to_string())]);
            assert_eq!(limit, Some(5));
            // Old-client wire shape (no after_id) → None, byte-identical.
            assert_eq!(after_id, None);
        }
        other => panic!("expected After, got {other:?}"),
    }
}

// ── task #537: record-id tie-breaker (after_id) ─────────────────────────────

/// `after(key, limit)` (the backward-compatible constructor) leaves
/// `after_id` as `None` and serializes to the EXACT same wire shape as
/// before task #537 — no `after_id` key emitted. Old clients that never
/// send a tie-breaker are byte-for-byte unchanged.
#[test]
fn after_without_tiebreaker_wire_shape_unchanged() {
    let key = vec![QueryValue::Int(30)];
    let p = Pagination::after(key.clone(), Some(2));
    assert_eq!(p.after_id(), None);

    let qv = to_qv(&p);
    // No `after_id` key — identical to the pre-#537 shape.
    assert_eq!(
        qv,
        mpack!({
            "mode": "After",
            "key": [ @ QueryValue::Int(30) ],
            "limit": 2_i64
        })
    );

    let back: Pagination = from_qv(qv);
    assert_eq!(back, p);
}

/// `after_with_id(key, limit, Some(id))` emits an `after_id` field on the
/// wire and round-trips it losslessly.
#[test]
fn after_with_tiebreaker_round_trip() {
    use shamir_types::types::record_id::RecordId;

    let id = RecordId::system("row-last-000");
    let key = vec![QueryValue::Int(20)];
    let p = Pagination::after_with_id(key.clone(), Some(3), Some(id));

    assert_eq!(p.after_id(), Some(&id));

    let qv = to_qv(&p);
    let back: Pagination = from_qv(qv);
    assert_eq!(back, p);
    // The tie-breaker survives the round-trip.
    assert_eq!(back.after_id(), Some(&id));
}

/// `after_with_id(.., None)` is equivalent to `after(..)` — no tie-breaker,
/// backward-compatible wire shape.
#[test]
fn after_with_id_none_equals_plain_after() {
    let key = vec![QueryValue::Int(7)];
    let a = Pagination::after_with_id(key.clone(), Some(1), None);
    let b = Pagination::after(key, Some(1));
    assert_eq!(a, b);
    assert_eq!(to_qv(&a), to_qv(&b));
}

/// `keyset()` returns the seek tuple and limit for `After`, and `None` for
/// every other variant.
#[test]
fn keyset_accessor() {
    // After → Some((&[...], limit))
    let key = vec![QueryValue::Int(1), QueryValue::Int(2)];
    let p = Pagination::after(key.clone(), Some(3));
    let (k, limit) = p.keyset().expect("After should yield a keyset");
    assert_eq!(k, &key[..]);
    assert_eq!(limit, Some(3));

    // After with no limit
    let p = Pagination::after(vec![], None);
    let (k, limit) = p.keyset().unwrap();
    assert!(k.is_empty());
    assert_eq!(limit, None);

    // Non-After variants → None
    assert!(Pagination::None.keyset().is_none());
    assert!(Pagination::LimitOffset {
        limit: Some(10),
        offset: 0
    }
    .keyset()
    .is_none());
    assert!(Pagination::page(1, 10).keyset().is_none());
}

/// `After` is NOT the default (it is not `None`), so `is_none()` is false.
#[test]
fn after_is_not_none() {
    let p = Pagination::after(vec![QueryValue::Int(1)], None);
    assert!(!p.is_none());
}
