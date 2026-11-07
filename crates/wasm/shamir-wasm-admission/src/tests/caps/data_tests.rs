use crate::tests::support::{admit, limits_with, module, module_with_pages, narrow, probe, wide};
use crate::AdmissionError;

/// One function plus `n` passive element segments naming it.
fn element_segments(n: usize) -> String {
    format!("(func $f) {}", "(elem func $f)".repeat(n))
}

/// One function plus a single passive segment listing it `n` times.
fn element_items(n: usize) -> String {
    format!("(func $f) (elem func {})", "$f ".repeat(n))
}

#[test]
fn table_initial_elements_at_limit_admitted() {
    let limits = limits_with(|l| l.max_table_elements = 4);
    let report = admit(&module("(table 4 funcref)"), limits).unwrap();
    assert_eq!(report.table_count, 1);
}

#[test]
fn table_initial_elements_over_limit_rejected() {
    let limits = limits_with(|l| l.max_table_elements = 4);
    let err = admit(&module("(table 5 funcref)"), limits).unwrap_err();
    assert!(
        matches!(
            err,
            AdmissionError::TableTooLarge {
                limit: 4,
                observed: 5
            }
        ),
        "{err}"
    );
}

#[test]
fn table_maximum_elements_at_limit_admitted() {
    let limits = limits_with(|l| l.max_table_elements = 4);
    assert!(admit(&module("(table 1 4 funcref)"), limits).is_ok());
}

#[test]
fn table_maximum_elements_over_limit_rejected() {
    let limits = limits_with(|l| l.max_table_elements = 4);
    let err = admit(&module("(table 1 5 funcref)"), limits).unwrap_err();
    assert!(
        matches!(
            err,
            AdmissionError::TableTooLarge {
                limit: 4,
                observed: 5
            }
        ),
        "{err}"
    );
}

#[test]
fn element_segments_at_limit_admitted() {
    let k = narrow(probe().element_segment_count) + 2;
    let limits = limits_with(|l| l.max_element_segments = k);
    let report = admit(&module(&element_segments(2)), limits).unwrap();
    assert_eq!(report.element_segment_count, wide(k));
}

#[test]
fn element_segments_over_limit_rejected() {
    let k = narrow(probe().element_segment_count) + 2;
    let limits = limits_with(|l| l.max_element_segments = k);
    let err = admit(&module(&element_segments(3)), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyElementSegments { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn element_items_at_limit_admitted() {
    let k = narrow(probe().element_item_count) + 3;
    let limits = limits_with(|l| l.max_element_items_total = k);
    let report = admit(&module(&element_items(3)), limits).unwrap();
    assert_eq!(report.element_item_count, wide(k));
}

#[test]
fn element_items_over_limit_rejected() {
    let k = narrow(probe().element_item_count) + 3;
    let limits = limits_with(|l| l.max_element_items_total = k);
    let err = admit(&module(&element_items(4)), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyElementItems { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn data_segments_at_limit_admitted() {
    let k = narrow(probe().data_segment_count) + 2;
    let limits = limits_with(|l| l.max_data_segments = k);
    let report = admit(&module(&r#"(data "x")"#.repeat(2)), limits).unwrap();
    assert_eq!(report.data_segment_count, wide(k));
}

#[test]
fn data_segments_over_limit_rejected() {
    let k = narrow(probe().data_segment_count) + 2;
    let limits = limits_with(|l| l.max_data_segments = k);
    let err = admit(&module(&r#"(data "x")"#.repeat(3)), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::TooManyDataSegments { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn data_bytes_at_limit_admitted() {
    let k = narrow(probe().data_bytes_total) + 4;
    let limits = limits_with(|l| l.max_data_bytes_total = k);
    let report = admit(&module(r#"(data "abcd")"#), limits).unwrap();
    assert_eq!(report.data_bytes_total, wide(k));
}

#[test]
fn data_bytes_over_limit_rejected() {
    let k = narrow(probe().data_bytes_total) + 4;
    let limits = limits_with(|l| l.max_data_bytes_total = k);
    let err = admit(&module(r#"(data "abcde")"#), limits).unwrap_err();
    assert!(
        matches!(err, AdmissionError::DataTooLarge { limit, observed }
            if limit == wide(k) && observed == wide(k + 1)),
        "{err}"
    );
}

#[test]
fn memory_min_pages_at_limit_admitted() {
    let limits = limits_with(|l| l.max_memory_min_pages = 4);
    let report = admit(&module_with_pages(4), limits).unwrap();
    assert_eq!(report.memory_min_pages, 4);
}

#[test]
fn memory_min_pages_over_limit_rejected() {
    let limits = limits_with(|l| l.max_memory_min_pages = 4);
    let err = admit(&module_with_pages(5), limits).unwrap_err();
    assert!(
        matches!(
            err,
            AdmissionError::MemoryTooLarge {
                limit: 4,
                observed: 5
            }
        ),
        "{err}"
    );
}
