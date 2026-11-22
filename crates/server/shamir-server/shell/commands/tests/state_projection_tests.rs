//! Unit regression for the `data head` and `data count` projection seams
//! (admin-shell PR3, §4.4): the projected-row BYTE cap, the shared
//! MONOTONIC deadline and the count-path statistics propagation are
//! exercised through the deterministic seam — `project_head` takes its
//! deadline as a parameter, so the wall-time refusal is driven by an
//! ALREADY-EXPIRED deadline and the passing path by one far in the
//! future. No sleeps, no tokio runtime, no wall-clock thresholds: the
//! only clock read is the monotonic `tokio::time::Instant::now` that
//! builds the seam deadlines.

use std::time::Duration;

use shamir_query_types::read::{PaginationInfo, QueryRecord, QueryResult, QueryStats};
use shamir_types::types::value::QueryValue;

use super::super::state::{project_count, project_head};
use crate::shell::output::ShellError;

/// A direct map record with one large string field of `chars` characters.
fn big_row(chars: usize) -> QueryRecord {
    let mut fields = shamir_collections::new_map();
    fields.insert(String::from("payload"), QueryValue::Str("x".repeat(chars)));
    QueryRecord::Direct(QueryValue::Map(fields))
}

fn result_with(records: Vec<QueryRecord>) -> QueryResult {
    QueryResult {
        records,
        ..QueryResult::default()
    }
}

fn pagination(has_next: bool) -> PaginationInfo {
    PaginationInfo {
        total_count: None,
        total_pages: None,
        current_page: None,
        page_size: None,
        has_next,
        has_prev: false,
    }
}

/// The FIRST row that would overflow the byte cap stops the projection
/// and sets `bytes_truncated`; earlier rows stay in `rows` and the
/// row-truncation markers stay untouched by the byte cap.
#[test]
fn head_byte_cap_stops_projection_with_marker() {
    // Two ~150 KiB rows: the first fits, the second cannot (256 KiB cap).
    let result = result_with(vec![big_row(150 * 1024), big_row(150 * 1024)]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = project_head(&result, 2, 2, deadline).expect("projection succeeds");
    assert_eq!(
        out["rows"].as_array().map(Vec::len),
        Some(1),
        "exactly the rows that fit must be projected"
    );
    assert_eq!(out["bytes_truncated"].as_bool(), Some(true));
    assert_eq!(out["rows_truncated"].as_bool(), Some(false));
    assert!(out.get("rows_capped").is_none());
    assert_eq!(out["rows_returned"].as_str(), Some("1"));
}

/// An expired deadline (deterministic seam, no sleep) is the typed
/// `head_wall_time` outcome-unknown refusal — even with zero rows, the
/// projection never emits a partial success.
#[test]
fn head_expired_deadline_is_head_wall_time() {
    let result = result_with(vec![big_row(16)]);
    let expired = tokio::time::Instant::now() - Duration::from_secs(1);
    let error = project_head(&result, 1, 1, expired).expect_err("expired deadline refuses");
    assert_eq!(error, ShellError::HeadWallTime);
}

/// A deadline far in the future (deterministic seam) lets a normal small
/// result complete with both budget markers false.
#[test]
fn head_future_deadline_completes_with_clean_markers() {
    let result = result_with(vec![big_row(16)]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3600);
    let out = project_head(&result, 1, 1, deadline).expect("projection succeeds");
    assert_eq!(out["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(out["bytes_truncated"].as_bool(), Some(false));
    assert_eq!(out["rows_truncated"].as_bool(), Some(false));
}

/// `rows_truncated` requires BOTH a page that says more rows exist AND a
/// request above the hard row cap; `rows_capped` records pure clamping.
/// A dishonest peer's extra rows are dropped by the limit take.
#[test]
fn head_row_markers_follow_request_cap_and_pagination() {
    // Peer returns MORE records than the limit; the projection takes
    // exactly `limit` and never marks row truncation without the cap.
    let result = result_with((0..7).map(|_| big_row(4)).collect());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = project_head(&result, 5, 5, deadline).expect("projection succeeds");
    assert_eq!(out["rows"].as_array().map(Vec::len), Some(5));
    assert_eq!(out["rows_truncated"].as_bool(), Some(false));
    assert!(out.get("rows_capped").is_none());

    // Above the hard cap with a `has_next` page: both markers fire.
    let capped = QueryResult {
        pagination: Some(pagination(true)),
        ..result_with((0..100).map(|_| big_row(4)).collect())
    };
    let capped_out = project_head(&capped, 150, 100, deadline).expect("projection succeeds");
    assert_eq!(capped_out["rows_truncated"].as_bool(), Some(true));
    assert_eq!(capped_out["rows_capped"].as_bool(), Some(true));

    // Above the hard cap but a FINAL page (`has_next == false`): the
    // clamp is visible, row truncation is not claimed.
    let final_page = QueryResult {
        pagination: Some(pagination(false)),
        ..result_with((0..100).map(|_| big_row(4)).collect())
    };
    let final_out = project_head(&final_page, 150, 100, deadline).expect("projection succeeds");
    assert_eq!(final_out["rows_truncated"].as_bool(), Some(false));
    assert_eq!(final_out["rows_capped"].as_bool(), Some(true));
}

/// Count-path statistics propagation (engine dispatch ground truth):
/// the `__record_counter__` index marker is the O(1) counter, a scanned
/// statistic is a scan, and missing/foreign statistics stay `unknown`
/// WITH the fixed caveat — the shell never invents a path.
#[test]
fn count_path_follows_query_stats() {
    let row = || {
        let mut fields = shamir_collections::new_map();
        fields.insert(String::from("result"), QueryValue::Int(7));
        result_with(vec![QueryRecord::Direct(QueryValue::Map(fields))])
    };

    let mut counter = row();
    counter.stats = Some(QueryStats {
        index_used: Some(String::from("__record_counter__")),
        records_scanned: 0,
        records_returned: 1,
        execution_time_us: 1,
    });
    let out = project_count(&counter).expect("counter row projects");
    assert_eq!(out["count"].as_str(), Some("7"));
    assert_eq!(out["count_path"].as_str(), Some("counter"));
    assert!(out.get("caveat").is_none());

    let mut scan = row();
    scan.stats = Some(QueryStats {
        index_used: None,
        records_scanned: 7,
        records_returned: 1,
        execution_time_us: 2,
    });
    let out = project_count(&scan).expect("scan row projects");
    assert_eq!(out["count_path"].as_str(), Some("scan"));
    assert!(out.get("caveat").is_none());

    let out = project_count(&row()).expect("stats-free row projects");
    assert_eq!(out["count_path"].as_str(), Some("unknown"));
    assert!(
        out["caveat"]
            .as_str()
            .is_some_and(|text| text.contains("O(1) counter")),
        "an unknown count path must carry the fixed caveat"
    );
}
