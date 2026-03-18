//! Tests for Pagination and PaginationInfo

use crate::db::query::read::{Pagination, PaginationInfo};

// ============================================================================
// Pagination::resolve() tests
// ============================================================================

#[test]
fn test_limit_offset_resolve() {
    let p = Pagination::LimitOffset {
        limit: Some(10),
        offset: 20,
    };
    assert_eq!(p.resolve(), (20, Some(10)));
}

#[test]
fn test_limit_offset_no_limit_resolve() {
    let p = Pagination::LimitOffset {
        limit: None,
        offset: 5,
    };
    assert_eq!(p.resolve(), (5, None));
}

#[test]
fn test_page_resolve_page_1() {
    let p = Pagination::Page {
        page: 1,
        page_size: 10,
    };
    assert_eq!(p.resolve(), (0, Some(10)));
}

#[test]
fn test_page_resolve_page_2() {
    let p = Pagination::Page {
        page: 2,
        page_size: 10,
    };
    assert_eq!(p.resolve(), (10, Some(10)));
}

#[test]
fn test_page_resolve_page_3() {
    let p = Pagination::Page {
        page: 3,
        page_size: 25,
    };
    assert_eq!(p.resolve(), (50, Some(25)));
}

#[test]
fn test_page_resolve_page_0_saturates() {
    let p = Pagination::Page {
        page: 0,
        page_size: 10,
    };
    // page 0 → saturating_sub(1) = 0, so skip = 0
    assert_eq!(p.resolve(), (0, Some(10)));
}

#[test]
fn test_none_resolve() {
    let p = Pagination::None;
    assert_eq!(p.resolve(), (0, None));
}

// ============================================================================
// PaginationInfo::compute() tests
// ============================================================================

#[test]
fn test_compute_page_based_with_total() {
    let p = Pagination::Page {
        page: 2,
        page_size: 10,
    };
    let info = PaginationInfo::compute(&p, Some(25));

    assert_eq!(info.total_count, Some(25));
    assert_eq!(info.total_pages, Some(3)); // ceil(25/10)
    assert_eq!(info.current_page, Some(2));
    assert_eq!(info.page_size, Some(10));
    assert!(info.has_next); // page 2 of 3
    assert!(info.has_prev); // page > 1
}

#[test]
fn test_compute_page_based_last_page() {
    let p = Pagination::Page {
        page: 3,
        page_size: 10,
    };
    let info = PaginationInfo::compute(&p, Some(25));

    assert_eq!(info.total_count, Some(25));
    assert_eq!(info.total_pages, Some(3));
    assert_eq!(info.current_page, Some(3));
    assert!(!info.has_next); // skip=20 + 10 >= 25
    assert!(info.has_prev);
}

#[test]
fn test_compute_page_based_first_page() {
    let p = Pagination::Page {
        page: 1,
        page_size: 10,
    };
    let info = PaginationInfo::compute(&p, Some(25));

    assert_eq!(info.current_page, Some(1));
    assert!(info.has_next);
    assert!(!info.has_prev);
}

#[test]
fn test_compute_limit_offset_with_total() {
    let p = Pagination::LimitOffset {
        limit: Some(10),
        offset: 20,
    };
    let info = PaginationInfo::compute(&p, Some(50));

    assert_eq!(info.total_count, Some(50));
    assert_eq!(info.total_pages, Some(5)); // ceil(50/10)
    assert_eq!(info.current_page, None); // not page-based
    assert_eq!(info.page_size, Some(10));
    assert!(info.has_next); // 20 + 10 < 50
    assert!(info.has_prev); // offset > 0
}

#[test]
fn test_compute_without_total() {
    let p = Pagination::Page {
        page: 2,
        page_size: 10,
    };
    let info = PaginationInfo::compute(&p, None);

    assert_eq!(info.total_count, None);
    assert_eq!(info.total_pages, None);
    assert_eq!(info.current_page, Some(2));
    assert_eq!(info.page_size, Some(10));
    assert!(!info.has_next); // unknown without total
    assert!(info.has_prev);
}

#[test]
fn test_compute_without_total_with_has_next_hint() {
    let p = Pagination::Page {
        page: 2,
        page_size: 10,
    };
    let info = PaginationInfo::compute(&p, None).with_has_next(true);

    assert!(info.has_next);
}

#[test]
fn test_compute_none_pagination() {
    let p = Pagination::None;
    let info = PaginationInfo::compute(&p, Some(100));

    assert_eq!(info.total_count, Some(100));
    assert_eq!(info.total_pages, None);
    assert_eq!(info.current_page, None);
    assert_eq!(info.page_size, None);
    assert!(!info.has_next);
    assert!(!info.has_prev);
}

#[test]
fn test_compute_exact_page_boundary() {
    let p = Pagination::Page {
        page: 5,
        page_size: 10,
    };
    let info = PaginationInfo::compute(&p, Some(50));

    assert_eq!(info.total_pages, Some(5));
    assert!(!info.has_next); // 40 + 10 = 50, not < 50
}

// ============================================================================
// Pagination helpers
// ============================================================================

#[test]
fn test_pagination_is_none() {
    assert!(Pagination::None.is_none());
    assert!(Pagination::default().is_none());
    assert!(!Pagination::page(1, 10).is_none());
    assert!(!Pagination::LimitOffset { limit: Some(10), offset: 0 }.is_none());
}

// ============================================================================
// Parser: JSON → Pagination (tested via common_parser_tests)
// ============================================================================

#[test]
fn test_parse_page_based_query() {
    use crate::db::query::read::query_from_value;
    use crate::types::value::QueryValue;

    let json = r#"{
        "from": "users",
        "limit": { "page": 3, "page_size": 20 },
        "count_total": true
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let query = query_from_value(&value).unwrap();

    assert_eq!(query.from, crate::db::query::TableRef::new("users"));
    assert!(matches!(
        query.pagination,
        Pagination::Page { page: 3, page_size: 20 }
    ));
    assert!(query.count_total);
}

#[test]
fn test_parse_limit_offset_query_backward_compat() {
    use crate::db::query::read::query_from_value;
    use crate::types::value::QueryValue;

    let json = r#"{
        "from": "users",
        "limit": { "limit": 10, "offset": 20 }
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let query = query_from_value(&value).unwrap();

    assert!(matches!(
        query.pagination,
        Pagination::LimitOffset { limit: Some(10), offset: 20 }
    ));
    assert!(!query.count_total);
}

#[test]
fn test_parse_no_pagination() {
    use crate::db::query::read::query_from_value;
    use crate::types::value::QueryValue;

    let json = r#"{ "from": "users" }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let query = query_from_value(&value).unwrap();

    assert!(query.pagination.is_none());
}

#[test]
fn test_parse_count_total_false_explicit() {
    use crate::db::query::read::query_from_value;
    use crate::types::value::QueryValue;

    let json = r#"{
        "from": "users",
        "limit": { "page": 1, "page_size": 10 },
        "count_total": false
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let query = query_from_value(&value).unwrap();

    assert!(matches!(
        query.pagination,
        Pagination::Page { page: 1, page_size: 10 }
    ));
    assert!(!query.count_total);
}

#[test]
fn test_parse_limit_only_full_query() {
    use crate::db::query::read::query_from_value;
    use crate::types::value::QueryValue;

    let json = r#"{
        "from": "products",
        "limit": { "limit": 50 }
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let query = query_from_value(&value).unwrap();

    assert!(matches!(
        query.pagination,
        Pagination::LimitOffset { limit: Some(50), offset: 0 }
    ));
}

#[test]
fn test_parse_page_based_without_count_total() {
    use crate::db::query::read::query_from_value;
    use crate::types::value::QueryValue;

    let json = r#"{
        "from": "logs",
        "limit": { "page": 5, "page_size": 100 }
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let query = query_from_value(&value).unwrap();

    assert!(matches!(
        query.pagination,
        Pagination::Page { page: 5, page_size: 100 }
    ));
    assert!(!query.count_total);
}

#[test]
fn test_parse_page_1_edge_case() {
    use crate::db::query::read::query_from_value;
    use crate::types::value::QueryValue;

    let json = r#"{
        "from": "users",
        "where": { "op": "eq", "field": "active", "value": true },
        "limit": { "page": 1, "page_size": 25 },
        "count_total": true
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let query = query_from_value(&value).unwrap();

    assert!(matches!(
        query.pagination,
        Pagination::Page { page: 1, page_size: 25 }
    ));
    assert!(query.count_total);
    assert!(query.r#where.is_some());

    // Verify resolve for page 1
    let (skip, take) = query.pagination.resolve();
    assert_eq!(skip, 0);
    assert_eq!(take, Some(25));
}

#[test]
fn test_parse_offset_only_full_query() {
    use crate::db::query::read::query_from_value;
    use crate::types::value::QueryValue;

    let json = r#"{
        "from": "events",
        "limit": { "offset": 100 }
    }"#;

    let value: QueryValue = serde_json::from_str(json).unwrap();
    let query = query_from_value(&value).unwrap();

    assert!(matches!(
        query.pagination,
        Pagination::LimitOffset { limit: None, offset: 100 }
    ));
}
