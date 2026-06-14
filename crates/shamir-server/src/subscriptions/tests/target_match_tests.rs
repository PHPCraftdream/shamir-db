use std::sync::Arc;

use shamir_collections::TMap;
use shamir_db::core::interner::Interner;
use shamir_db::types::value::InnerValue;
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::subscribe::event_mask::EventMask;
use shamir_tx::ChangeOp;
use tokio::sync::OnceCell;

use crate::subscriptions::target_match::{mask_matches, matches_any};

/// Build a `(InnerValue, Arc<OnceCell<Interner>>)` tuple from flat key-value
/// pairs, suitable for passing as `inner_decoded` to `matches_any`.
fn make_inner(fields: &[(&str, InnerValue)]) -> (InnerValue, Arc<OnceCell<Interner>>) {
    let interner = Interner::new();
    let mut map: TMap<_, InnerValue> = TMap::default();
    for (field, val) in fields {
        let key = interner.touch_ind(*field).expect("intern field").into_key();
        map.insert(key, val.clone());
    }
    let cell = OnceCell::new();
    cell.set(interner).unwrap();
    (InnerValue::Map(map), Arc::new(cell))
}

#[test]
fn mask_all_matches_everything() {
    assert!(mask_matches(&EventMask::All, &ChangeOp::Put));
    assert!(mask_matches(&EventMask::All, &ChangeOp::Delete));
}

#[test]
fn mask_put_matches_only_put() {
    assert!(mask_matches(&EventMask::Put, &ChangeOp::Put));
    assert!(!mask_matches(&EventMask::Put, &ChangeOp::Delete));
}

#[test]
fn mask_delete_matches_only_delete() {
    assert!(!mask_matches(&EventMask::Delete, &ChangeOp::Put));
    assert!(mask_matches(&EventMask::Delete, &ChangeOp::Delete));
}

#[test]
fn matches_any_filters_by_repo_table_and_mask() {
    let targets = vec![
        ("repo_a".into(), "users".into(), EventMask::Put, None),
        ("repo_b".into(), "logs".into(), EventMask::All, None),
    ];
    assert!(matches_any(
        &targets,
        "repo_a",
        "users",
        &ChangeOp::Put,
        None
    ));
    assert!(!matches_any(
        &targets,
        "repo_a",
        "users",
        &ChangeOp::Delete,
        None
    ));
    assert!(matches_any(
        &targets,
        "repo_b",
        "logs",
        &ChangeOp::Put,
        None
    ));
    assert!(matches_any(
        &targets,
        "repo_b",
        "logs",
        &ChangeOp::Delete,
        None
    ));
    assert!(!matches_any(
        &targets,
        "repo_a",
        "other",
        &ChangeOp::Put,
        None
    ));
}

#[test]
fn matches_any_rejects_table_collision_across_repos() {
    let targets = vec![("repo_a".into(), "users".into(), EventMask::All, None)];
    assert!(matches_any(
        &targets,
        "repo_a",
        "users",
        &ChangeOp::Put,
        None
    ));
    assert!(!matches_any(
        &targets,
        "repo_b",
        "users",
        &ChangeOp::Put,
        None
    ));
}

#[test]
fn put_with_matching_filter_delivered() {
    let filter = Filter::Eq {
        field: vec!["status".to_string()],
        value: FilterValue::String("active".to_string()),
    };
    let targets = vec![("repo".into(), "users".into(), EventMask::Put, Some(filter))];
    let record = make_inner(&[
        ("status", InnerValue::Str("active".to_string())),
        ("name", InnerValue::Str("alice".to_string())),
    ]);
    assert!(matches_any(
        &targets,
        "repo",
        "users",
        &ChangeOp::Put,
        Some(&record),
    ));
}

#[test]
fn put_with_non_matching_filter_skipped() {
    let filter = Filter::Eq {
        field: vec!["status".to_string()],
        value: FilterValue::String("active".to_string()),
    };
    let targets = vec![("repo".into(), "users".into(), EventMask::Put, Some(filter))];
    let record = make_inner(&[
        ("status", InnerValue::Str("inactive".to_string())),
        ("name", InnerValue::Str("bob".to_string())),
    ]);
    assert!(!matches_any(
        &targets,
        "repo",
        "users",
        &ChangeOp::Put,
        Some(&record),
    ));
}

#[test]
fn delete_with_filter_delivered_regardless() {
    let filter = Filter::Eq {
        field: vec!["status".to_string()],
        value: FilterValue::String("active".to_string()),
    };
    let targets = vec![("repo".into(), "users".into(), EventMask::All, Some(filter))];
    assert!(matches_any(
        &targets,
        "repo",
        "users",
        &ChangeOp::Delete,
        None,
    ));
}

#[test]
fn put_without_filter_delivered() {
    let targets = vec![("repo".into(), "users".into(), EventMask::Put, None)];
    let record = make_inner(&[("status", InnerValue::Str("anything".to_string()))]);
    assert!(matches_any(
        &targets,
        "repo",
        "users",
        &ChangeOp::Put,
        Some(&record),
    ));
}
