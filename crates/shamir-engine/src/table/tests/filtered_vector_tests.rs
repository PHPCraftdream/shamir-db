//! V3.1 / P3 leaf 3.1 — unit tests for filtered-ANN plan recognition
//! (`try_extract_filtered_vector_query` / `build_residual` / `resolve_oversample`).
//!
//! Pure unit tests on the recognition functions; the end-to-end filtered-ANN
//! behaviour lives in [`super::filtered_ann_tests`]. These were previously
//! inlined in `filtered_vector.rs` and moved here per the repo convention
//! (CLAUDE.md: tests live in `tests/`, not inline `mod tests` in impl files).

use shamir_query_types::filter::{Filter, FilterValue};

use crate::table::filtered_vector::{resolve_oversample, try_extract_filtered_vector_query};

fn vec_sim() -> Filter {
    Filter::VectorSimilarity {
        field: vec!["embedding".into()],
        query: vec![1.0, 0.0],
        k: 10,
        ef_search: None,
        oversample: None,
    }
}

fn eq(field: &str, val: &str) -> Filter {
    Filter::Eq {
        field: vec![field.into()],
        value: FilterValue::String(val.into()),
    }
}

#[test]
fn bare_vector_returns_none() {
    assert!(try_extract_filtered_vector_query(&vec_sim()).is_none());
}

#[test]
fn and_without_vector_returns_none() {
    let f = Filter::And {
        filters: vec![eq("city", "NYC"), eq("zip", "10001")],
    };
    assert!(try_extract_filtered_vector_query(&f).is_none());
}

#[test]
fn and_with_two_vectors_returns_none() {
    let f = Filter::And {
        filters: vec![vec_sim(), vec_sim()],
    };
    assert!(try_extract_filtered_vector_query(&f).is_none());
}

#[test]
fn and_with_one_vector_and_one_pred_extracts() {
    let f = Filter::And {
        filters: vec![vec_sim(), eq("city", "NYC")],
    };
    let fvq = try_extract_filtered_vector_query(&f).expect("must recognise");
    assert_eq!(fvq.k, 10);
    assert_eq!(fvq.residual, Some(eq("city", "NYC")));
}

#[test]
fn and_with_one_vector_and_two_preds_packs_residual_as_and() {
    let f = Filter::And {
        filters: vec![vec_sim(), eq("city", "NYC"), eq("zip", "10001")],
    };
    let fvq = try_extract_filtered_vector_query(&f).expect("must recognise");
    let residual = fvq.residual.expect("residual present");
    assert!(
        matches!(&residual, Filter::And { filters } if filters.len() == 2),
        "expected And residual with 2 conjuncts, got {residual:?}"
    );
}

#[test]
fn oversample_default_is_two() {
    assert_eq!(resolve_oversample(None), 2.0);
}

#[test]
fn oversample_clamped_to_min_one() {
    assert_eq!(resolve_oversample(Some(0.5)), 1.0);
    assert_eq!(resolve_oversample(Some(0.0)), 1.0);
}

#[test]
fn oversample_explicit_preserved() {
    assert_eq!(resolve_oversample(Some(3.5)), 3.5);
}
