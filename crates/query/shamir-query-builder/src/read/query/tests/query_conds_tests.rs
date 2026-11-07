//! Tests for the standalone `Conds` builder, the full "kitchen sink"
//! query, and the validating `try_build` sibling of `build()`.

use shamir_types::mpack;

use crate::filter::{self, FilterExt};
use crate::query::{Conds, Query, QueryBuildError};

use super::query_tests::assert_wire;

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
        mpack!({
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

// ============================================================================
// try_build — validating sibling of build() (items §1/§2)
// ============================================================================

#[test]
fn try_build_having_without_group_by_returns_err() {
    let err = Query::from("orders")
        .having(filter::gt("n", 5))
        .try_build()
        .unwrap_err();
    assert_eq!(err, QueryBuildError::HavingWithoutGroupBy);
}

#[test]
fn try_build_having_with_group_by_ok() {
    let rq = Query::from("orders")
        .group_by("status")
        .having(filter::gt("n", 5))
        .try_build()
        .expect("having with group_by must build");
    // Sanity: the group_by + having landed on the wire.
    assert!(rq.group_by.is_some());
}

#[test]
fn build_having_without_group_by_still_lenient() {
    // Regression guard: the legacy build() path is unchanged and permissive —
    // it does NOT surface the HavingWithoutGroupBy check.
    let rq = Query::from("orders").having(filter::gt("n", 5)).build();
    assert!(
        rq.group_by.is_some(),
        "build() still emits the empty-group query"
    );
}

#[test]
fn try_build_page_zero_returns_err() {
    let err = Query::from("items").page(0, 25).try_build().unwrap_err();
    assert_eq!(err, QueryBuildError::InvalidPage { page: 0 });
}

#[test]
fn try_build_page_size_zero_returns_err() {
    let err = Query::from("items").page(1, 0).try_build().unwrap_err();
    assert_eq!(err, QueryBuildError::InvalidPageSize { page_size: 0 });
}

#[test]
fn try_build_valid_page_ok() {
    let rq = Query::from("items")
        .page(1, 25)
        .try_build()
        .expect("page=1 page_size>0 must build");
    assert!(matches!(
        rq.pagination,
        shamir_query_types::read::Pagination::Page { .. }
    ));
}
