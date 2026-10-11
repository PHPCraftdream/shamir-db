//! Pure server-owned limit model and wire compatibility regressions.

use serde::Serialize;

use crate::batch::{BatchError, BatchLimits, ExecPolicy, LimitKind};

// Serialize the old DTO, not today's struct: the wire compatibility is the subject.
#[derive(Serialize)]
struct OldLimits {
    max_queries: usize,
    max_dependency_depth: usize,
    max_execution_time_secs: u64,
    max_result_size: usize,
    max_nesting_depth: usize,
    max_iterations: usize,
}

fn assert_old_limits(limits: &BatchLimits) {
    assert_eq!(limits.max_queries, 17);
    assert_eq!(limits.max_dependency_depth, 6);
    assert_eq!(limits.max_execution_time_secs, 12);
    assert_eq!(limits.max_result_size, 2048);
    assert_eq!(limits.max_nesting_depth, 3);
    assert_eq!(limits.max_iterations, 42);
    assert_new_defaults(limits);
}

fn assert_new_defaults(limits: &BatchLimits) {
    assert_eq!(limits.max_total_queries, 10_000);
    assert_eq!(limits.max_total_work, 1_000_000);
    assert_eq!(limits.max_page_size, 100_000);
    assert_eq!(limits.max_offset, 1_000_000);
}

#[test]
fn old_six_field_named_msgpack_defaults_new_fields() {
    let old = OldLimits {
        max_queries: 17,
        max_dependency_depth: 6,
        max_execution_time_secs: 12,
        max_result_size: 2048,
        max_nesting_depth: 3,
        max_iterations: 42,
    };
    let bytes = rmp_serde::to_vec_named(&old).unwrap();
    let limits: BatchLimits = rmp_serde::from_slice(&bytes).unwrap();
    assert_old_limits(&limits);
    assert_new_defaults(&BatchLimits::default());
}

#[test]
fn old_six_field_compact_msgpack_defaults_new_fields() {
    // A tuple encodes the original six-field sequence even with to_vec_named.
    let old = (17usize, 6usize, 12u64, 2048usize, 3usize, 42usize);
    let bytes = rmp_serde::to_vec_named(&old).unwrap();
    let limits: BatchLimits = rmp_serde::from_slice(&bytes).unwrap();
    assert_old_limits(&limits);
}

#[test]
fn clamp_minimum_in_both_orders_and_idempotence() {
    let input = BatchLimits {
        max_queries: 1,
        max_dependency_depth: 20,
        max_execution_time_secs: 3,
        max_result_size: 40,
        max_nesting_depth: 5,
        max_iterations: 60,
        max_total_queries: 7,
        max_total_work: 80,
        max_page_size: 9,
        max_offset: 100,
    };
    let cap = BatchLimits {
        max_queries: 10,
        max_dependency_depth: 2,
        max_execution_time_secs: 30,
        max_result_size: 4,
        max_nesting_depth: 50,
        max_iterations: 6,
        max_total_queries: 70,
        max_total_work: 8,
        max_page_size: 90,
        max_offset: 10,
    };
    let expected = BatchLimits {
        max_queries: 1,
        max_dependency_depth: 2,
        max_execution_time_secs: 3,
        max_result_size: 4,
        max_nesting_depth: 5,
        max_iterations: 6,
        max_total_queries: 7,
        max_total_work: 8,
        max_page_size: 9,
        max_offset: 10,
    };
    assert_eq!(input.clamped_to(&cap), expected);
    assert_eq!(cap.clamped_to(&input), expected);
    assert_eq!(expected.clamped_to(&cap), expected);
    assert_eq!(expected.clamped_to(&input), expected);
    assert_eq!(expected.clamped_to(&expected), expected);
}

#[test]
fn finite_ceiling_and_embedded_policy() {
    let expected = BatchLimits {
        max_queries: 1000,
        max_dependency_depth: 1000,
        max_execution_time_secs: 3600,
        max_result_size: 100 * 1024 * 1024,
        max_nesting_depth: 64,
        max_iterations: 10_000,
        max_total_queries: 10_000,
        max_total_work: 1_000_000,
        max_page_size: 100_000,
        max_offset: 1_000_000,
    };
    let huge = BatchLimits {
        max_queries: usize::MAX,
        max_dependency_depth: usize::MAX,
        max_execution_time_secs: u64::MAX,
        max_result_size: usize::MAX,
        max_nesting_depth: usize::MAX,
        max_iterations: usize::MAX,
        max_total_queries: usize::MAX,
        max_total_work: u64::MAX,
        max_page_size: u64::MAX,
        max_offset: u64::MAX,
    };
    let small = BatchLimits {
        max_queries: 1,
        max_dependency_depth: 2,
        max_execution_time_secs: 3,
        max_result_size: 4,
        max_nesting_depth: 0,
        max_iterations: 6,
        max_total_queries: 7,
        max_total_work: 8,
        max_page_size: 9,
        max_offset: 10,
    };
    assert_eq!(BatchLimits::CEILING, expected);
    assert_eq!(BatchLimits::CEILING.clamped_to(&huge), expected);
    assert_eq!(huge.clamped_to(&BatchLimits::CEILING), expected);
    assert_eq!(BatchLimits::CEILING.clamped_to(&small), small);
    assert_eq!(small.clamped_to(&BatchLimits::CEILING), small);
    assert_eq!(
        BatchLimits::default().clamped_to(&expected),
        BatchLimits::default()
    );
    assert_eq!(ExecPolicy::EMBEDDED.caps, expected);
    assert!(!ExecPolicy::EMBEDDED.enforce_result_size);
}

#[test]
fn limit_exceeded_display_reports_kind_actual_and_max() {
    for (kind, name) in [
        (LimitKind::TotalQueries, "TotalQueries"),
        (LimitKind::Work, "Work"),
        (LimitKind::Iterations, "Iterations"),
        (LimitKind::Depth, "Depth"),
        (LimitKind::PageSize, "PageSize"),
        (LimitKind::Offset, "Offset"),
        (LimitKind::ResultBytes, "ResultBytes"),
    ] {
        let error = BatchError::LimitExceeded {
            kind,
            actual: 123,
            max: 45,
        };
        assert_eq!(
            error.to_string(),
            format!("Limit exceeded: {name}, actual: 123 (max: 45)")
        );
        assert_eq!(error.code(), None);
    }
}
