//! Tests for the `Query` builder — every clause type is verified against
//! exact wire JSON and round-tripped through serde.

use serde_json::{json, Value};
use shamir_query_types::read::{OrderByItem, ReadQuery};

use crate::filter::{self, FilterExt};
use crate::query::{Conds, Query};
use crate::select;
use crate::val::*;

// ── helpers ────────────────────────────────────────────────────────

/// Serialize the built `ReadQuery` to JSON, assert equality, then
/// round-trip back and assert structural equality.
fn assert_wire(rq: ReadQuery, expected: Value) {
    let got = serde_json::to_value(&rq).expect("serialize");
    assert_eq!(got, expected, "wire JSON mismatch");
    let back: ReadQuery = serde_json::from_value(got).expect("deserialize");
    assert_eq!(back, rq, "round-trip mismatch");
}

// ── from / with_repo ───────────────────────────────────────────────

#[test]
fn test_from_default_repo() {
    let rq = Query::from("users").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {
                "items": [{"type": "all"}],
                "distinct": false
            }
        }),
    );
}

#[test]
fn test_with_repo() {
    let rq = Query::with_repo("analytics", "events").build();
    assert_wire(
        rq,
        json!({
            "from": ["analytics", "events"],
            "select": {
                "items": [{"type": "all"}],
                "distinct": false
            }
        }),
    );
}

// ── select (strings) ───────────────────────────────────────────────

#[test]
fn test_select_strings() {
    let rq = Query::from("users").select(["id", "name", "age"]).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {
                "items": [
                    {"type": "field", "path": ["id"]},
                    {"type": "field", "path": ["name"]},
                    {"type": "field", "path": ["age"]}
                ],
                "distinct": false
            }
        }),
    );
}

// ── select (SelectItem mix) ────────────────────────────────────────

#[test]
fn test_select_items() {
    let rq = Query::from("users")
        .select([
            select::field("name"),
            select::func("up", "strings/upper", [col("name")]),
        ])
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {
                "items": [
                    {"type": "field", "path": ["name"]},
                    {
                        "type": "function",
                        "name": "strings/upper",
                        "args": [{"$ref": ["name"]}],
                        "alias": "up"
                    }
                ],
                "distinct": false
            }
        }),
    );
}

// ── distinct ───────────────────────────────────────────────────────

#[test]
fn test_distinct() {
    let rq = Query::from("users").select(["email"]).distinct().build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {
                "items": [{"type": "field", "path": ["email"]}],
                "distinct": true
            }
        }),
    );
}

// ── where_eq (single) ──────────────────────────────────────────────

#[test]
fn test_where_eq() {
    let rq = Query::from("users").where_eq("status", "active").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "eq",
                "field": ["status"],
                "value": "active"
            }
        }),
    );
}

// ── where_ne ───────────────────────────────────────────────────────

#[test]
fn test_where_ne() {
    let rq = Query::from("users").where_ne("status", "banned").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "ne",
                "field": ["status"],
                "value": "banned"
            }
        }),
    );
}

// ── where_gt / gte / lt / lte ──────────────────────────────────────

#[test]
fn test_where_gt() {
    let rq = Query::from("users").where_gt("age", 18).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "gt", "field": ["age"], "value": 18}
        }),
    );
}

#[test]
fn test_where_gte() {
    let rq = Query::from("users").where_gte("age", 18).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "gte", "field": ["age"], "value": 18}
        }),
    );
}

#[test]
fn test_where_lt() {
    let rq = Query::from("users").where_lt("age", 65).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "lt", "field": ["age"], "value": 65}
        }),
    );
}

#[test]
fn test_where_lte() {
    let rq = Query::from("users").where_lte("score", 100).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "lte", "field": ["score"], "value": 100}
        }),
    );
}

// ── AND chaining (two where_eq -> And) ─────────────────────────────

#[test]
fn test_and_chaining() {
    let rq = Query::from("users")
        .where_eq("status", "active")
        .where_gt("age", 18)
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "and",
                "filters": [
                    {"op": "eq", "field": ["status"], "value": "active"},
                    {"op": "gt", "field": ["age"], "value": 18}
                ]
            }
        }),
    );
}

// ── where_in / where_not_in ────────────────────────────────────────

#[test]
fn test_where_in() {
    let rq = Query::from("users")
        .where_in("role", ["admin", "mod"])
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "in",
                "field": ["role"],
                "values": ["admin", "mod"]
            }
        }),
    );
}

#[test]
fn test_where_not_in() {
    let rq = Query::from("users")
        .where_not_in("role", ["banned", "suspended"])
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "not_in",
                "field": ["role"],
                "values": ["banned", "suspended"]
            }
        }),
    );
}

// ── like / ilike / regex ───────────────────────────────────────────

#[test]
fn test_like() {
    let rq = Query::from("users").like("name", "Al%").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "like", "field": ["name"], "pattern": "Al%"}
        }),
    );
}

#[test]
fn test_ilike() {
    let rq = Query::from("users").ilike("email", "%@GMAIL%").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "i_like", "field": ["email"], "pattern": "%@GMAIL%"}
        }),
    );
}

#[test]
fn test_regex() {
    let rq = Query::from("users").regex("name", "^A.*z$").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "regex", "field": ["name"], "pattern": "^A.*z$"}
        }),
    );
}

// ── null / existence ───────────────────────────────────────────────

#[test]
fn test_where_null() {
    let rq = Query::from("users").where_null("deleted_at").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "is_null", "field": ["deleted_at"]}
        }),
    );
}

#[test]
fn test_where_not_null() {
    let rq = Query::from("users").where_not_null("email").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "is_not_null", "field": ["email"]}
        }),
    );
}

#[test]
fn test_where_exists() {
    let rq = Query::from("users").where_exists("avatar").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "exists", "field": ["avatar"]}
        }),
    );
}

#[test]
fn test_where_not_exists() {
    let rq = Query::from("users").where_not_exists("temp_flag").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "not_exists", "field": ["temp_flag"]}
        }),
    );
}

// ── containment ────────────────────────────────────────────────────

#[test]
fn test_where_contains() {
    let rq = Query::from("posts").where_contains("tags", "rust").build();
    assert_wire(
        rq,
        json!({
            "from": "posts",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {"op": "contains", "field": ["tags"], "value": "rust"}
        }),
    );
}

#[test]
fn test_where_contains_any() {
    let rq = Query::from("posts")
        .where_contains_any("tags", ["rust", "go"])
        .build();
    assert_wire(
        rq,
        json!({
            "from": "posts",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "contains_any",
                "field": ["tags"],
                "values": ["rust", "go"]
            }
        }),
    );
}

#[test]
fn test_where_contains_all() {
    let rq = Query::from("posts")
        .where_contains_all("tags", ["rust", "async"])
        .build();
    assert_wire(
        rq,
        json!({
            "from": "posts",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "contains_all",
                "field": ["tags"],
                "values": ["rust", "async"]
            }
        }),
    );
}

// ── between ────────────────────────────────────────────────────────

#[test]
fn test_where_between() {
    let rq = Query::from("users").where_between("age", 18, 65).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "between",
                "field": ["age"],
                "from": 18,
                "to": 65
            }
        }),
    );
}

// ── fts ────────────────────────────────────────────────────────────

#[test]
fn test_fts() {
    let rq = Query::from("articles")
        .fts("body", "rust async", "match")
        .build();
    assert_wire(
        rq,
        json!({
            "from": "articles",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "fts",
                "field": ["body"],
                "query": "rust async",
                "mode": "match"
            }
        }),
    );
}

// ── where_ (drop-in filter) ───────────────────────────────────────

#[test]
fn test_where_drop_in() {
    let f = filter::eq("x", 1).and(filter::eq("y", 2));
    let rq = Query::from("t").where_(f).build();
    assert_wire(
        rq,
        json!({
            "from": "t",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "and",
                "filters": [
                    {"op": "eq", "field": ["x"], "value": 1},
                    {"op": "eq", "field": ["y"], "value": 2}
                ]
            }
        }),
    );
}

// ── or_where_eq ────────────────────────────────────────────────────

#[test]
fn test_or_where_eq() {
    let rq = Query::from("users")
        .where_eq("role", "admin")
        .or_where_eq("role", "superadmin")
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "or",
                "filters": [
                    {"op": "eq", "field": ["role"], "value": "admin"},
                    {"op": "eq", "field": ["role"], "value": "superadmin"}
                ]
            }
        }),
    );
}

// ── or_where_ (drop-in OR) ────────────────────────────────────────

#[test]
fn test_or_where_drop_in() {
    let rq = Query::from("users")
        .where_eq("a", 1)
        .or_where_(filter::eq("b", 2))
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "or",
                "filters": [
                    {"op": "eq", "field": ["a"], "value": 1},
                    {"op": "eq", "field": ["b"], "value": 2}
                ]
            }
        }),
    );
}

// ── where_group (nested AND group) ─────────────────────────────────

#[test]
fn test_where_group() {
    // status = 'active' AND (age > 18 AND age < 65)
    let rq = Query::from("users")
        .where_eq("status", "active")
        .where_group(|g| g.where_gt("age", 18).where_lt("age", 65))
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "and",
                "filters": [
                    {"op": "eq", "field": ["status"], "value": "active"},
                    {
                        "op": "and",
                        "filters": [
                            {"op": "gt", "field": ["age"], "value": 18},
                            {"op": "lt", "field": ["age"], "value": 65}
                        ]
                    }
                ]
            }
        }),
    );
}

// ── where_group_or (nested OR group) ───────────────────────────────

#[test]
fn test_where_group_or() {
    // status = 'active' OR (role = 'admin' OR vip = true)
    // where_group_or OR-combines the group with the accumulator.
    let rq = Query::from("users")
        .where_eq("status", "active")
        .where_group_or(|g| g.where_eq("role", "admin").or_where_eq("vip", true))
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "or",
                "filters": [
                    {"op": "eq", "field": ["status"], "value": "active"},
                    {
                        "op": "or",
                        "filters": [
                            {"op": "eq", "field": ["role"], "value": "admin"},
                            {"op": "eq", "field": ["vip"], "value": true}
                        ]
                    }
                ]
            }
        }),
    );
}

#[test]
fn test_where_group_or_codeigniter_pattern() {
    // The CodeIgniter pattern: (status = 'active') AND (role = 'admin' OR vip = true)
    // Use where_group with inner or_where_eq to get AND-combined OR group.
    let rq = Query::from("users")
        .where_eq("status", "active")
        .where_group(|g| g.where_eq("role", "admin").or_where_eq("vip", true))
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "where": {
                "op": "and",
                "filters": [
                    {"op": "eq", "field": ["status"], "value": "active"},
                    {
                        "op": "or",
                        "filters": [
                            {"op": "eq", "field": ["role"], "value": "admin"},
                            {"op": "eq", "field": ["vip"], "value": true}
                        ]
                    }
                ]
            }
        }),
    );
}

// ── group_by ───────────────────────────────────────────────────────

#[test]
fn test_group_by_single() {
    let rq = Query::from("orders")
        .select([select::field("city"), select::count_all("n")])
        .group_by("city")
        .build();
    assert_wire(
        rq,
        json!({
            "from": "orders",
            "select": {
                "items": [
                    {"type": "field", "path": ["city"]},
                    {"type": "count_all", "alias": "n"}
                ],
                "distinct": false
            },
            "group_by": {
                "fields": [["city"]]
            }
        }),
    );
}

#[test]
fn test_group_by_many() {
    let rq = Query::from("orders")
        .group_by_many(["city", "country"])
        .build();
    assert_wire(
        rq,
        json!({
            "from": "orders",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "group_by": {
                "fields": [["city"], ["country"]]
            }
        }),
    );
}

// ── having ─────────────────────────────────────────────────────────

#[test]
fn test_group_by_having() {
    let rq = Query::from("orders")
        .select([select::field("city"), select::count_all("n")])
        .group_by("city")
        .having(filter::gt("n", 10))
        .build();
    assert_wire(
        rq,
        json!({
            "from": "orders",
            "select": {
                "items": [
                    {"type": "field", "path": ["city"]},
                    {"type": "count_all", "alias": "n"}
                ],
                "distinct": false
            },
            "group_by": {
                "fields": [["city"]],
                "having": {"op": "gt", "field": ["n"], "value": 10}
            }
        }),
    );
}

#[test]
fn test_having_without_group_by() {
    // Edge case: having without group_by fields still creates GroupBy.
    let rq = Query::from("t").having(filter::gt("cnt", 5)).build();
    assert_wire(
        rq,
        json!({
            "from": "t",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "group_by": {
                "fields": [],
                "having": {"op": "gt", "field": ["cnt"], "value": 5}
            }
        }),
    );
}

// ── order_by ───────────────────────────────────────────────────────

#[test]
fn test_order_by_asc() {
    let rq = Query::from("users").order_by_asc("name").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "order_by": {
                "items": [{"field": ["name"], "direction": "asc"}]
            }
        }),
    );
}

#[test]
fn test_order_by_desc() {
    let rq = Query::from("users").order_by_desc("age").build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "order_by": {
                "items": [{"field": ["age"], "direction": "desc"}]
            }
        }),
    );
}

#[test]
fn test_order_by_item_with_nulls() {
    let rq = Query::from("users")
        .order_by(OrderByItem::desc("score").nulls_last())
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "order_by": {
                "items": [{
                    "field": ["score"],
                    "direction": "desc",
                    "nulls": "last"
                }]
            }
        }),
    );
}

#[test]
fn test_order_by_multiple() {
    let rq = Query::from("users")
        .order_by_desc("age")
        .order_by_asc("name")
        .build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "order_by": {
                "items": [
                    {"field": ["age"], "direction": "desc"},
                    {"field": ["name"], "direction": "asc"}
                ]
            }
        }),
    );
}

// ── limit / offset ─────────────────────────────────────────────────

#[test]
fn test_limit() {
    let rq = Query::from("users").limit(20).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "pagination": {"mode": "LimitOffset", "limit": 20, "offset": 0}
        }),
    );
}

#[test]
fn test_limit_offset() {
    let rq = Query::from("users").limit(20).offset(40).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "pagination": {"mode": "LimitOffset", "limit": 20, "offset": 40}
        }),
    );
}

#[test]
fn test_offset_then_limit() {
    // Order-independent: offset then limit still produces LimitOffset.
    let rq = Query::from("users").offset(10).limit(5).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "pagination": {"mode": "LimitOffset", "limit": 5, "offset": 10}
        }),
    );
}

// ── page ───────────────────────────────────────────────────────────

#[test]
fn test_page() {
    let rq = Query::from("users").page(3, 25).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "pagination": {"mode": "Page", "page": 3, "page_size": 25}
        }),
    );
}

// ── count_total ────────────────────────────────────────────────────

#[test]
fn test_count_total() {
    let rq = Query::from("users").count_total(true).build();
    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {"items": [{"type": "all"}], "distinct": false},
            "count_total": true
        }),
    );
}

// ── From<Query> for ReadQuery ──────────────────────────────────────

#[test]
fn test_into_read_query() {
    let q = Query::from("users").where_eq("id", 1);
    let rq: ReadQuery = q.into();
    let expected = Query::from("users").where_eq("id", 1).build();
    assert_eq!(rq, expected);
}

// ── Conds standalone ───────────────────────────────────────────────

#[test]
fn test_conds_empty() {
    let c = Conds::new();
    assert!(c.into_filter().is_none());
}

#[test]
fn test_conds_single() {
    let c = Conds::new().where_eq("x", 1);
    let f = c.into_filter().expect("should have filter");
    let expected = filter::eq("x", 1);
    assert_eq!(f, expected);
}

#[test]
fn test_conds_and_chain() {
    let c = Conds::new().where_eq("x", 1).where_eq("y", 2);
    let f = c.into_filter().expect("should have filter");
    let expected = filter::eq("x", 1).and(filter::eq("y", 2));
    assert_eq!(f, expected);
}

// ── kitchen sink ───────────────────────────────────────────────────

#[test]
fn test_kitchen_sink() {
    // Use where_group (AND-combined) with inner or_where_eq to get the
    // CodeIgniter-style pattern: ...AND (vip OR premium).
    let rq = Query::from("users")
        .select(["id", "name", "age"])
        .distinct()
        .where_eq("status", "active")
        .where_gt("age", 18)
        .where_in("role", ["admin", "mod"])
        .like("name", "Al%")
        .where_group(|g| g.where_eq("vip", true).or_where_eq("premium", true))
        .group_by("city")
        .having(filter::gt("cnt", 3))
        .order_by_desc("age")
        .order_by_asc("name")
        .limit(20)
        .offset(40)
        .count_total(true)
        .build();

    assert_wire(
        rq,
        json!({
            "from": "users",
            "select": {
                "items": [
                    {"type": "field", "path": ["id"]},
                    {"type": "field", "path": ["name"]},
                    {"type": "field", "path": ["age"]}
                ],
                "distinct": true
            },
            "where": {
                "op": "and",
                "filters": [
                    {"op": "eq", "field": ["status"], "value": "active"},
                    {"op": "gt", "field": ["age"], "value": 18},
                    {"op": "in", "field": ["role"], "values": ["admin", "mod"]},
                    {"op": "like", "field": ["name"], "pattern": "Al%"},
                    {
                        "op": "or",
                        "filters": [
                            {"op": "eq", "field": ["vip"], "value": true},
                            {"op": "eq", "field": ["premium"], "value": true}
                        ]
                    }
                ]
            },
            "group_by": {
                "fields": [["city"]],
                "having": {"op": "gt", "field": ["cnt"], "value": 3}
            },
            "order_by": {
                "items": [
                    {"field": ["age"], "direction": "desc"},
                    {"field": ["name"], "direction": "asc"}
                ]
            },
            "pagination": {"mode": "LimitOffset", "limit": 20, "offset": 40},
            "count_total": true
        }),
    );
}
