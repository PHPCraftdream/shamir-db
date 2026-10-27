//! Page-based pagination: `page: 0` resolves and echoes as page 1, offsets
//! that overflow `u64` saturate in `resolve` and are rejected by `try_resolve`,
//! and `PaginationInfo::compute` for each mode.

use crate::read::{Pagination, PaginationInfo};

#[test]
fn rv_query_types_b5653a74_page_zero_resolves_as_page_one() {
    let zero = Pagination::page(0, 10);
    let one = Pagination::page(1, 10);
    assert_eq!(zero.resolve(), (0, Some(10)));
    assert_eq!(zero.resolve(), one.resolve());
    assert_eq!(zero.try_resolve().unwrap(), one.try_resolve().unwrap());
}

#[test]
fn rv_query_types_b5653a74_page_zero_info_echoes_page_one() {
    let info = PaginationInfo::compute(&Pagination::page(0, 10), Some(25));
    assert_eq!(info.current_page, Some(1));
    assert_eq!(info.total_pages, Some(3));
    assert!(!info.has_prev);
}

#[test]
fn rv_query_types_b5653a74_page_offset_overflow_saturates_and_rejects() {
    let p = Pagination::page(u64::MAX, u64::MAX);
    assert_eq!(p.resolve(), (u64::MAX, Some(u64::MAX)));
    assert!(p.try_resolve().is_err());
}

#[test]
fn rv_query_types_b53ec5ce_pagination_info_limit_offset_middle_page() {
    let p = Pagination::LimitOffset {
        limit: Some(10),
        offset: 20,
    };
    let info = PaginationInfo::compute(&p, Some(25));
    assert_eq!(info.total_count, Some(25));
    assert_eq!(info.total_pages, Some(3));
    assert_eq!(info.current_page, None);
    assert_eq!(info.page_size, Some(10));
    assert!(info.has_prev);
    assert!(!info.has_next);
}

#[test]
fn rv_query_types_b53ec5ce_pagination_info_limit_offset_first_page_has_next() {
    let p = Pagination::LimitOffset {
        limit: Some(10),
        offset: 0,
    };
    let info = PaginationInfo::compute(&p, Some(25));
    assert!(!info.has_prev);
    assert!(info.has_next);
}

#[test]
fn rv_query_types_b53ec5ce_pagination_info_page_without_total() {
    let info = PaginationInfo::compute(&Pagination::page(2, 10), None);
    assert_eq!(info.total_count, None);
    assert_eq!(info.total_pages, None);
    assert_eq!(info.current_page, Some(2));
    assert_eq!(info.page_size, Some(10));
    assert!(info.has_prev);
    assert!(!info.has_next);
}

#[test]
fn rv_query_types_b53ec5ce_pagination_info_none_reports_total_only() {
    let info = PaginationInfo::compute(&Pagination::None, Some(7));
    assert_eq!(info.total_count, Some(7));
    assert_eq!(info.page_size, None);
    assert_eq!(info.current_page, None);
    assert!(!info.has_prev);
    assert!(!info.has_next);
}
