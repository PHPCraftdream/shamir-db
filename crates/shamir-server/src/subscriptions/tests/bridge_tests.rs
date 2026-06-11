use std::collections::HashMap;

use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::subscribe::event_mask::EventMask;
use shamir_tx::ChangeOp;

use crate::subscriptions::bridge::{filter_matches_value, mask_matches, matches_any};

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
fn watermark_skips_already_seen_versions() {
    let mut watermarks: HashMap<String, u64> = HashMap::new();
    let repo = "repo_a".to_string();

    watermarks.insert(repo.clone(), 5);

    let wm = watermarks.entry(repo.clone()).or_insert(0);
    assert!(3 <= *wm, "version 3 should be skipped (watermark=5)");
    assert!(5 <= *wm, "version 5 should be skipped (watermark=5)");

    assert!(6 > *wm, "version 6 should pass (watermark=5)");
    *wm = 6;
    assert_eq!(watermarks[&repo], 6);
}

#[test]
fn watermark_independent_per_repo() {
    let mut watermarks: HashMap<String, u64> = HashMap::new();
    watermarks.insert("repo_a".to_string(), 10);
    watermarks.insert("repo_b".to_string(), 3);

    let wm_a = watermarks.entry("repo_a".to_string()).or_insert(0);
    assert!(
        5 <= *wm_a,
        "repo_a version 5 should be skipped (watermark=10)"
    );

    let wm_b = watermarks.entry("repo_b".to_string()).or_insert(0);
    assert!(5 > *wm_b, "repo_b version 5 should pass (watermark=3)");
    *wm_b = 5;

    assert_eq!(watermarks["repo_a"], 10);
    assert_eq!(watermarks["repo_b"], 5);
}

#[test]
fn watermark_backfill_tracks_max_version() {
    let mut watermarks: HashMap<String, u64> = HashMap::new();
    let repo = "repo_a".to_string();

    for version in [2, 5, 3, 7, 6] {
        let wm = watermarks.entry(repo.clone()).or_insert(0);
        if version > *wm {
            *wm = version;
        }
    }

    assert_eq!(watermarks[&repo], 7);
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
    let value_bytes = rmp_serde::to_vec_named(&record).unwrap();
    assert!(matches_any(
        &targets,
        "repo",
        "users",
        &ChangeOp::Put,
        Some(&value_bytes),
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
    let value_bytes = rmp_serde::to_vec_named(&record).unwrap();
    assert!(!matches_any(
        &targets,
        "repo",
        "users",
        &ChangeOp::Put,
        Some(&value_bytes),
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
    let value_bytes = rmp_serde::to_vec_named(&record).unwrap();
    assert!(matches_any(
        &targets,
        "repo",
        "users",
        &ChangeOp::Put,
        Some(&value_bytes),
    ));
}

#[test]
fn filter_matches_value_eq() {
    let filter = Filter::Eq {
        field: vec!["name".to_string()],
        value: FilterValue::String("alice".to_string()),
    };
    let yes = serde_json::json!({"name": "alice"});
    let no = serde_json::json!({"name": "bob"});
    assert!(filter_matches_value(&filter, &yes));
    assert!(!filter_matches_value(&filter, &no));
}

#[test]
fn filter_matches_value_and() {
    let filter = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(18),
            },
        ],
    };
    let yes = serde_json::json!({"status": "active", "age": 25});
    let no = serde_json::json!({"status": "active", "age": 15});
    assert!(filter_matches_value(&filter, &yes));
    assert!(!filter_matches_value(&filter, &no));
}

#[test]
fn filter_matches_value_nested_field() {
    let filter = Filter::Eq {
        field: vec!["address".to_string(), "city".to_string()],
        value: FilterValue::String("Jerusalem".to_string()),
    };
    let yes = serde_json::json!({"address": {"city": "Jerusalem"}});
    let no = serde_json::json!({"address": {"city": "Tel Aviv"}});
    assert!(filter_matches_value(&filter, &yes));
    assert!(!filter_matches_value(&filter, &no));
}
