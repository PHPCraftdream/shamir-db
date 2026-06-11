use shamir_query_types::filter::{Filter, FilterValue};

use crate::subscriptions::filter_eval::filter_matches_value;

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
