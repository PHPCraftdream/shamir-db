//! Tests for BatchPlanner using JSON input
//!
//! All tests use JSON format with map syntax where alias is the key.

use serde_json::json;

use crate::db::query::batch::{BatchLimits, BatchPlanner, BatchRequest};

fn parse_request(json: serde_json::Value) -> BatchRequest {
    serde_json::from_value(json).expect("Failed to parse batch request")
}

#[test]
fn test_plan_empty() {
    let json = json!({
        "queries": {}
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 0);
    assert_eq!(plan.aliases.len(), 0);
}

#[test]
fn test_plan_single_query() {
    let json = json!({
        "queries": {
            "users": {
                "from": "users"
            }
        }
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
        "queries": {
            "users": { "from": "users" },
            "products": { "from": "products" },
            "orders": { "from": "orders" }
        }
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
        "queries": {
            "users": { "from": "users" },
            "orders": {
                "from": "orders",
                "where": {
                    "op": "eq",
                    "field": ["user_id"],
                    "value": { "$query": "users" }
                }
            }
        }
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
        "queries": {
            "users": { "from": "users" },
            "products": { "from": "products" },
            "orders": {
                "from": "orders",
                "where": {
                    "op": "and",
                    "filters": [
                        {
                            "op": "eq",
                            "field": ["user_id"],
                            "value": { "$query": "users" }
                        },
                        {
                            "op": "eq",
                            "field": ["product_id"],
                            "value": { "$query": "products" }
                        }
                    ]
                }
            },
            "stats": {
                "from": "stats",
                "where": {
                    "op": "eq",
                    "field": ["order_count"],
                    "value": { "$query": "orders" }
                }
            }
        }
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
fn test_plan_unknown_alias() {
    let json = json!({
        "queries": {
            "orders": {
                "from": "orders",
                "where": {
                    "op": "eq",
                    "field": ["user_id"],
                    "value": { "$query": "nonexistent" }
                }
            }
        }
    });

    let request = parse_request(json);
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(
        matches!(err, crate::db::query::batch::BatchError::UnknownAlias { alias, .. } if alias == "nonexistent")
    );
}

#[test]
fn test_plan_too_many_queries() {
    let mut queries = serde_json::Map::new();
    for i in 0..60 {
        queries.insert(format!("q{}", i), json!({ "from": "table" }));
    }

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
        "queries": {
            "a": { "from": "t" },
            "b": { "from": "t" },
            "c": { "from": "t" },
            "d": { "from": "t" }
        },
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
        "queries": {
            "a": {
                "from": "a",
                "where": {
                    "op": "eq",
                    "field": ["x"],
                    "value": { "$query": "c" }
                }
            },
            "b": {
                "from": "b",
                "where": {
                    "op": "eq",
                    "field": ["x"],
                    "value": { "$query": "a" }
                }
            },
            "c": {
                "from": "c",
                "where": {
                    "op": "eq",
                    "field": ["x"],
                    "value": { "$query": "b" }
                }
            }
        }
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
        "queries": {
            "self_ref": {
                "from": "t",
                "where": {
                    "op": "eq",
                    "field": ["x"],
                    "value": { "$query": "self_ref" }
                }
            }
        }
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
    let mut queries = serde_json::Map::new();

    queries.insert("q0".to_string(), json!({ "from": "t" }));

    for i in 1..15 {
        queries.insert(
            format!("q{}", i),
            json!({
                "from": "t",
                "where": {
                    "op": "eq",
                    "field": ["x"],
                    "value": { "$query": format!("q{}", i - 1) }
                }
            }),
        );
    }

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
        "queries": {
            "users": { "from": "users" },
            "orders": {
                "from": "orders",
                "where": {
                    "op": "in",
                    "field": ["user_id"],
                    "values": [
                        { "$query": "users" },
                        42
                    ]
                }
            }
        }
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
        "queries": {
            "users": { "from": "users" },
            "admins": { "from": "admins" },
            "results": {
                "from": "results",
                "where": {
                    "op": "or",
                    "filters": [
                        {
                            "op": "eq",
                            "field": ["user_id"],
                            "value": { "$query": "users" }
                        },
                        {
                            "op": "eq",
                            "field": ["admin_id"],
                            "value": { "$query": "admins" }
                        }
                    ]
                }
            }
        }
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
        "queries": {
            "a": { "from": "t" },
            "b": {
                "from": "t",
                "where": {
                    "op": "eq",
                    "field": ["x"],
                    "value": { "$query": "a" }
                }
            },
            "c": {
                "from": "t",
                "where": {
                    "op": "eq",
                    "field": ["x"],
                    "value": { "$query": "a" }
                }
            },
            "d": {
                "from": "t",
                "where": {
                    "op": "and",
                    "filters": [
                        {
                            "op": "eq",
                            "field": ["x"],
                            "value": { "$query": "b" }
                        },
                        {
                            "op": "eq",
                            "field": ["y"],
                            "value": { "$query": "c" }
                        }
                    ]
                }
            }
        }
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
        "queries": {
            "users": { "from": "users" },
            "orders": {
                "from": "orders",
                "where": {
                    "op": "eq",
                    "field": ["user_id"],
                    "value": {
                        "$query": "users",
                        "path": "[0].id"
                    }
                }
            }
        }
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
        "queries": {
            "users": {
                "from": "users",
                "return_result": true
            },
            "internal": {
                "from": "internal",
                "return_result": false
            }
        },
        "return_all": false,
        "return_only": ["users"]
    });

    let request = parse_request(json);
    assert_eq!(request.queries.len(), 2);
    assert!(request.queries.get("users").unwrap().return_result);
    assert!(!request.queries.get("internal").unwrap().return_result);
    assert!(!request.return_all);
    assert_eq!(request.return_only, Some(vec!["users".to_string()]));
}

#[test]
fn test_plan_transactional_batch() {
    let json = json!({
        "name": "user_order_transaction",
        "transactional": true,
        "queries": {
            "users": { "from": "users" },
            "orders": {
                "from": "orders",
                "where": {
                    "op": "eq",
                    "field": ["user_id"],
                    "value": { "$query": "users[0].id" }
                }
            }
        }
    });

    let request = parse_request(json);
    assert!(request.transactional);
    assert_eq!(request.name, Some("user_order_transaction".to_string()));
}

#[test]
fn test_plan_not_filter() {
    let json = json!({
        "queries": {
            "active_users": { "from": "users" },
            "inactive_users": {
                "from": "users",
                "where": {
                    "op": "not",
                    "filter": {
                        "op": "eq",
                        "field": ["id"],
                        "value": { "$query": "active_users[0].id" }
                    }
                }
            }
        }
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["active_users"]);
    assert_eq!(plan.stages[1], vec!["inactive_users"]);
}

// ============================================================================
// WRITE OPERATIONS TESTS
// ============================================================================

#[test]
fn test_plan_insert_operation() {
    let json = json!({
        "queries": {
            "insert_user": {
                "insert_into": "users",
                "values": [
                    { "name": "Alice", "email": "alice@example.com" }
                ]
            }
        }
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0], vec!["insert_user"]);
    assert!(plan.dependencies["insert_user"].is_empty());
}

#[test]
fn test_plan_update_operation() {
    let json = json!({
        "queries": {
            "users": { "from": "users" },
            "update_orders": {
                "update": "orders",
                "where": {
                    "op": "eq",
                    "field": ["user_id"],
                    "value": { "$query": "users[0].id" }
                },
                "set": { "status": "processed" }
            }
        }
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["update_orders"]);
    assert!(plan.dependencies["update_orders"].contains("users"));
}

#[test]
fn test_plan_set_operation() {
    let json = json!({
        "queries": {
            "users": { "from": "users" },
            "set_user": {
                "set": "users",
                "key": { "id": { "$query": "users[0].id" } },
                "value": { "status": "updated" }
            }
        }
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["set_user"]);
    assert!(plan.dependencies["set_user"].contains("users"));
}

#[test]
fn test_plan_delete_operation() {
    let json = json!({
        "queries": {
            "inactive_users": {
                "from": "users",
                "where": {
                    "op": "eq",
                    "field": ["status"],
                    "value": "inactive"
                }
            },
            "delete_orders": {
                "delete_from": "orders",
                "where": {
                    "op": "in",
                    "field": ["user_id"],
                    "values": [{ "$query": "inactive_users[].id" }]
                }
            }
        }
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["inactive_users"]);
    assert_eq!(plan.stages[1], vec!["delete_orders"]);
    assert!(plan.dependencies["delete_orders"].contains("inactive_users"));
}

#[test]
fn test_plan_set_with_value_reference() {
    let json = json!({
        "queries": {
            "source": { "from": "source_table" },
            "set_target": {
                "set": "target_table",
                "key": { "id": 1 },
                "value": {
                    "name": { "$query": "source[0].name" },
                    "status": "copied"
                }
            }
        }
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert!(plan.dependencies["set_target"].contains("source"));
}

#[test]
fn test_plan_insert_with_value_reference() {
    let json = json!({
        "queries": {
            "user": {
                "from": "users",
                "where": { "op": "eq", "field": ["id"], "value": 1 }
            },
            "insert_order": {
                "insert_into": "orders",
                "values": [
                    {
                        "user_id": { "$query": "user[0].id" },
                        "product": "Widget"
                    }
                ]
            }
        }
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["user"]);
    assert_eq!(plan.stages[1], vec!["insert_order"]);
}

#[test]
fn test_plan_mixed_read_and_write() {
    let json = json!({
        "queries": {
            "users": { "from": "users" },
            "products": { "from": "products" },
            "insert_order": {
                "insert_into": "orders",
                "values": [{ "user_id": 1, "product_id": 1 }]
            },
            "update_inventory": {
                "update": "inventory",
                "where": { "op": "eq", "field": ["product_id"], "value": 1 },
                "set": { "quantity": 0 }
            }
        }
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0].len(), 4);
}

#[test]
fn test_plan_update_without_filter() {
    let json = json!({
        "queries": {
            "update_all": {
                "update": "users",
                "set": { "status": "active" }
            }
        }
    });

    let request = parse_request(json);
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert!(plan.dependencies["update_all"].is_empty());
}

#[test]
fn test_plan_write_operations_serialization() {
    let json = json!({
        "queries": {
            "insert": {
                "insert_into": "users",
                "values": [{ "name": "Test" }]
            },
            "update": {
                "update": "users",
                "where": { "op": "eq", "field": ["name"], "value": "Test" },
                "set": { "name": "Updated" }
            },
            "set": {
                "set": "users",
                "key": { "id": 1 },
                "value": { "name": "Set" }
            },
            "delete": {
                "delete_from": "users",
                "where": { "op": "eq", "field": ["id"], "value": 999 }
            }
        }
    });

    let request = parse_request(json);
    assert_eq!(request.queries.len(), 4);

    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();
    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0].len(), 4);
}
