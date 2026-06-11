use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::subscribe::event_mask::EventMask;
use shamir_tx::ChangeOp;

use crate::subscriptions::target_match::{mask_matches, matches_any};

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
    let record = serde_json::json!({
        "status": "active",
        "name": "alice"
    });
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
    let record = serde_json::json!({
        "status": "inactive",
        "name": "bob"
    });
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
    let record = serde_json::json!({"status": "anything"});
    assert!(matches_any(
        &targets,
        "repo",
        "users",
        &ChangeOp::Put,
        Some(&record),
    ));
}
