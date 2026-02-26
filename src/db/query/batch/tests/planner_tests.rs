//! Tests for BatchPlanner using JSON input
//!
//! All tests use JSON format to demonstrate real-world usage.

use serde_json::json;

use crate::db::query::batch::{BatchLimits, BatchPlanner, BatchRequest};

fn parse_request(json: serde_json::Value) -> BatchRequest {
    serde_json::from_value(json).expect("Failed to parse batch request")
}

#[test]
fn test_plan_empty() {
    let json = json!({
        "queries": []
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 0);
    assert_eq!(plan.aliases.len(), 0);
}

#[test]
fn test_plan_single_query() {
    let json = json!({
        "queries": [
            {
                "alias": "users",
                "query": {
                    "from": "users"
                }
            }
        ]
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.aliases, vec!["users"]);
}

#[test]
fn test_plan_parallel_queries() {
    let json = json!({
        "queries": [
            {
                "alias": "users",
                "query": { "from": "users" }
            },
            {
                "alias": "products",
                "query": { "from": "products" }
            },
            {
                "alias": "orders",
                "query": { "from": "orders" }
            }
        ]
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0].len(), 3);

    assert!(plan.dependencies["users"].is_empty());
    assert!(plan.dependencies["products"].is_empty());
    assert!(plan.dependencies["orders"].is_empty());
}

#[test]
fn test_plan_sequential_dependencies() {
    let json = json!({
        "queries": [
            {
                "alias": "users",
                "query": { "from": "users" }
            },
            {
                "alias": "orders",
                "query": {
                    "from": "orders",
                    "where": {
                        "op": "eq",
                        "field": "user_id",
                        "value": { "$query": "users" }
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["orders"]);

    assert!(plan.dependencies["users"].is_empty());
    assert!(plan.dependencies["orders"].contains("users"));
}

#[test]
fn test_plan_complex_dependencies() {
    let json = json!({
        "queries": [
            {
                "alias": "users",
                "query": { "from": "users" }
            },
            {
                "alias": "products",
                "query": { "from": "products" }
            },
            {
                "alias": "orders",
                "query": {
                    "from": "orders",
                    "where": {
                        "op": "and",
                        "filters": [
                            {
                                "op": "eq",
                                "field": "user_id",
                                "value": { "$query": "users" }
                            },
                            {
                                "op": "eq",
                                "field": "product_id",
                                "value": { "$query": "products" }
                            }
                        ]
                    }
                }
            },
            {
                "alias": "stats",
                "query": {
                    "from": "stats",
                    "where": {
                        "op": "eq",
                        "field": "order_count",
                        "value": { "$query": "orders" }
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 3);

    assert_eq!(plan.stages[0].len(), 2);
    assert!(plan.stages[0].contains(&"users".to_string()));
    assert!(plan.stages[0].contains(&"products".to_string()));

    assert_eq!(plan.stages[1], vec!["orders"]);
    assert_eq!(plan.stages[2], vec!["stats"]);
}

#[test]
fn test_plan_duplicate_alias() {
    let json = json!({
        "queries": [
            {
                "alias": "users",
                "query": { "from": "users" }
            },
            {
                "alias": "users",
                "query": { "from": "other" }
            }
        ]
    });

    let request = parse_request(json);
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(
        matches!(err, crate::db::query::batch::BatchError::DuplicateAlias { alias } if alias == "users")
    );
}

#[test]
fn test_plan_unknown_alias() {
    let json = json!({
        "queries": [
            {
                "alias": "orders",
                "query": {
                    "from": "orders",
                    "where": {
                        "op": "eq",
                        "field": "user_id",
                        "value": { "$query": "nonexistent" }
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(
        matches!(err, crate::db::query::batch::BatchError::UnknownAlias { alias, .. } if alias == "nonexistent")
    );
}

#[test]
fn test_plan_too_many_queries() {
    let queries: Vec<_> = (0..60)
        .map(|i| {
            json!({
                "alias": format!("q{}", i),
                "query": { "from": "table" }
            })
        })
        .collect();

    let json = json!({
        "queries": queries
    });

    let request = parse_request(json);
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(matches!(
        err,
        crate::db::query::batch::BatchError::TooManyQueries { count: 60, max: 50 }
    ));
}

#[test]
fn test_plan_custom_limits() {
    let json = json!({
        "queries": [
            { "alias": "a", "query": { "from": "t" } },
            { "alias": "b", "query": { "from": "t" } },
            { "alias": "c", "query": { "from": "t" } },
            { "alias": "d", "query": { "from": "t" } }
        ],
        "limits": {
            "max_queries": 3,
            "max_dependency_depth": 10,
            "max_execution_time_secs": 30,
            "max_result_size": 10000000
        }
    });

    let request = parse_request(json);
    let err = BatchPlanner::plan(&request.queries, &request.limits).unwrap_err();
    assert!(matches!(
        err,
        crate::db::query::batch::BatchError::TooManyQueries { count: 4, max: 3 }
    ));
}

#[test]
fn test_plan_circular_dependency() {
    let json = json!({
        "queries": [
            {
                "alias": "a",
                "query": {
                    "from": "a",
                    "where": {
                        "op": "eq",
                        "field": "x",
                        "value": { "$query": "c" }
                    }
                }
            },
            {
                "alias": "b",
                "query": {
                    "from": "b",
                    "where": {
                        "op": "eq",
                        "field": "x",
                        "value": { "$query": "a" }
                    }
                }
            },
            {
                "alias": "c",
                "query": {
                    "from": "c",
                    "where": {
                        "op": "eq",
                        "field": "x",
                        "value": { "$query": "b" }
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(matches!(
        err,
        crate::db::query::batch::BatchError::CircularDependency { .. }
    ));
}

#[test]
fn test_plan_self_dependency() {
    let json = json!({
        "queries": [
            {
                "alias": "self_ref",
                "query": {
                    "from": "t",
                    "where": {
                        "op": "eq",
                        "field": "x",
                        "value": { "$query": "self_ref" }
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(matches!(
        err,
        crate::db::query::batch::BatchError::CircularDependency { .. }
    ));
}

#[test]
fn test_plan_dependency_depth() {
    let queries: Vec<_> = (0..15)
        .map(|i| {
            if i == 0 {
                json!({
                    "alias": format!("q{}", i),
                    "query": { "from": "t" }
                })
            } else {
                json!({
                    "alias": format!("q{}", i),
                    "query": {
                        "from": "t",
                        "where": {
                            "op": "eq",
                            "field": "x",
                            "value": { "$query": format!("q{}", i - 1) }
                        }
                    }
                })
            }
        })
        .collect();

    let json = json!({
        "queries": queries,
        "limits": {
            "max_queries": 50,
            "max_dependency_depth": 10,
            "max_execution_time_secs": 30,
            "max_result_size": 10000000
        }
    });

    let request = parse_request(json);
    let err = BatchPlanner::plan(&request.queries, &request.limits).unwrap_err();
    assert!(matches!(
        err,
        crate::db::query::batch::BatchError::TooDeep { depth: 14, max: 10 }
    ));
}

#[test]
fn test_plan_mixed_in_filter() {
    let json = json!({
        "queries": [
            {
                "alias": "users",
                "query": { "from": "users" }
            },
            {
                "alias": "orders",
                "query": {
                    "from": "orders",
                    "where": {
                        "op": "in",
                        "field": "user_id",
                        "values": [
                            { "$query": "users" },
                            42
                        ]
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["orders"]);
}

#[test]
fn test_plan_or_filter() {
    let json = json!({
        "queries": [
            {
                "alias": "users",
                "query": { "from": "users" }
            },
            {
                "alias": "admins",
                "query": { "from": "admins" }
            },
            {
                "alias": "results",
                "query": {
                    "from": "results",
                    "where": {
                        "op": "or",
                        "filters": [
                            {
                                "op": "eq",
                                "field": "user_id",
                                "value": { "$query": "users" }
                            },
                            {
                                "op": "eq",
                                "field": "admin_id",
                                "value": { "$query": "admins" }
                            }
                        ]
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0].len(), 2);
    assert_eq!(plan.stages[1], vec!["results"]);
}

#[test]
fn test_plan_diamond_dependency() {
    let json = json!({
        "queries": [
            {
                "alias": "a",
                "query": { "from": "t" }
            },
            {
                "alias": "b",
                "query": {
                    "from": "t",
                    "where": {
                        "op": "eq",
                        "field": "x",
                        "value": { "$query": "a" }
                    }
                }
            },
            {
                "alias": "c",
                "query": {
                    "from": "t",
                    "where": {
                        "op": "eq",
                        "field": "x",
                        "value": { "$query": "a" }
                    }
                }
            },
            {
                "alias": "d",
                "query": {
                    "from": "t",
                    "where": {
                        "op": "and",
                        "filters": [
                            {
                                "op": "eq",
                                "field": "x",
                                "value": { "$query": "b" }
                            },
                            {
                                "op": "eq",
                                "field": "y",
                                "value": { "$query": "c" }
                            }
                        ]
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 3);
    assert_eq!(plan.stages[0], vec!["a"]);
    assert_eq!(plan.stages[1].len(), 2);
    assert_eq!(plan.stages[2], vec!["d"]);
}

#[test]
fn test_plan_with_query_path() {
    let json = json!({
        "queries": [
            {
                "alias": "users",
                "query": { "from": "users" }
            },
            {
                "alias": "orders",
                "query": {
                    "from": "orders",
                    "where": {
                        "op": "eq",
                        "field": "user_id",
                        "value": {
                            "$query": "users",
                            "path": "[0].id"
                        }
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["orders"]);
}

#[test]
fn test_plan_return_flags() {
    let json = json!({
        "queries": [
            {
                "alias": "users",
                "query": { "from": "users" },
                "return_result": true
            },
            {
                "alias": "internal",
                "query": { "from": "internal" },
                "return_result": false
            }
        ],
        "return_all": false,
        "return_only": ["users"]
    });

    let request = parse_request(json);
    assert_eq!(request.queries.len(), 2);
    assert!(request.queries[0].return_result);
    assert!(!request.queries[1].return_result);
    assert!(!request.return_all);
    assert_eq!(request.return_only, Some(vec!["users".to_string()]));
}

#[test]
fn test_plan_transactional_batch() {
    let json = json!({
        "name": "user_order_transaction",
        "transactional": true,
        "queries": [
            {
                "alias": "users",
                "query": { "from": "users" }
            },
            {
                "alias": "orders",
                "query": {
                    "from": "orders",
                    "where": {
                        "op": "eq",
                        "field": "user_id",
                        "value": { "$query": "users[0].id" }
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    assert!(request.transactional);
    assert_eq!(request.name, Some("user_order_transaction".to_string()));
}

#[test]
fn test_plan_not_filter() {
    let json = json!({
        "queries": [
            {
                "alias": "active_users",
                "query": { "from": "users" }
            },
            {
                "alias": "inactive_users",
                "query": {
                    "from": "users",
                    "where": {
                        "op": "not",
                        "filter": {
                            "op": "eq",
                            "field": "id",
                            "value": { "$query": "active_users[0].id" }
                        }
                    }
                }
            }
        ]
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["active_users"]);
    assert_eq!(plan.stages[1], vec!["inactive_users"]);
}
