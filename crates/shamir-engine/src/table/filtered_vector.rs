//! Filtered-ANN plan recognition (V3.1 / P3 leaf 3.1).
//!
//! Recognises the query shape `And([VectorSimilarity, ...residual-predicates])`
//! and compiles it into a [`FilteredVectorQuery`] — an internal planning form
//! that carries the vector half plus the residual predicate tree. This does
//! NOT modify the public [`Filter`] enum; it is a planner-internal rewrite
//! that feeds the post-filter + adaptive-oversample execution path in
//! [`read_exec`].
//!
//! Only the exact shape `And` with exactly ONE `VectorSimilarity` conjunct is
//! recognised. A bare `VectorSimilarity` (no `And`) is left untouched so the
//! pre-existing index2 fast path keeps its behaviour (back-compat). Multiple
//! `VectorSimilarity` in one `And` is NOT a filtered-ANN query and returns
//! `None` (falls through to the legacy full-scan path).

use shamir_query_types::filter::{FieldPath, Filter};

/// Internal planning form for `And([VectorSimilarity, ...residual])`.
///
/// Produced by [`try_extract_filtered_vector_query`] and consumed by the
/// post-filter oversample-retry loop in `read_exec::read_filtered_vector_scan`.
/// The vector half is captured by copy (small: field path + query Vec + knobs);
/// the residual predicates are captured by clone (the filter tree is `Clone`).
#[derive(Debug, Clone)]
pub(crate) struct FilteredVectorQuery {
    pub field: FieldPath,
    pub query: Vec<f32>,
    pub k: u32,
    pub ef_search: Option<u32>,
    pub oversample: Option<f32>,
    /// The non-vector conjuncts, re-packaged:
    /// - 0 remaining → `None` (degenerates to bare vector; shouldn't happen
    ///   post-recognition, but handled defensively)
    /// - 1 remaining → that filter as-is
    /// - ≥2 remaining → `Filter::And(filters)`
    pub residual: Option<Filter>,
}

/// Default oversample multiplier when the request does not specify one.
///
/// 2× is the standard post-filter starting point: cheap (doubles the ANN
/// candidate set), and the retry loop widens by another 2× per iteration if
/// the residual predicate is more selective than the oversample assumed.
pub(crate) const DEFAULT_OVERSAMPLE: f32 = 2.0;

/// Minimum oversample multiplier (clamped). Below 1.0 makes no sense for a
/// post-filter path — it would under-fetch relative to `k`.
pub(crate) const MIN_OVERSAMPLE: f32 = 1.0;

/// Recognise `And([VectorSimilarity, ...residual])` and extract a
/// [`FilteredVectorQuery`].
///
/// Returns `None` when:
/// - the filter is not an `And`, OR
/// - the `And` has no `VectorSimilarity` conjunct, OR
/// - the `And` has MORE than one `VectorSimilarity` conjunct (ambiguous —
///   which vector field wins? leave to the legacy path).
///
/// A bare `VectorSimilarity` (not wrapped in `And`) also returns `None` —
/// the caller keeps the existing index2 fast path for that shape.
pub(crate) fn try_extract_filtered_vector_query(filter: &Filter) -> Option<FilteredVectorQuery> {
    let conjuncts = match filter {
        Filter::And { filters } => filters,
        _ => return None,
    };

    // Locate the (single) VectorSimilarity conjunct.
    let mut vec_idx: Option<usize> = None;
    for (i, f) in conjuncts.iter().enumerate() {
        if matches!(f, Filter::VectorSimilarity { .. }) {
            if vec_idx.is_some() {
                // More than one VectorSimilarity — not a filtered-ANN query.
                return None;
            }
            vec_idx = Some(i);
        }
    }
    let vec_idx = vec_idx?;

    let vec_filter = match &conjuncts[vec_idx] {
        Filter::VectorSimilarity {
            field,
            query,
            k,
            ef_search,
            oversample,
        } => FilteredVectorQuery {
            field: field.clone(),
            query: query.clone(),
            k: *k,
            ef_search: *ef_search,
            oversample: *oversample,
            residual: build_residual(conjuncts, vec_idx),
        },
        // Unreachable: vec_idx points at a VectorSimilarity.
        _ => return None,
    };
    Some(vec_filter)
}

/// Build the residual predicate from the non-vector conjuncts.
fn build_residual(conjuncts: &[Filter], consumed_idx: usize) -> Option<Filter> {
    let remaining: Vec<Filter> = conjuncts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != consumed_idx)
        .map(|(_, f)| f.clone())
        .collect();
    match remaining.len() {
        0 => None,
        1 => Some(remaining.into_iter().next().unwrap()),
        _ => Some(Filter::And { filters: remaining }),
    }
}

/// Clamp the oversample multiplier to a sane range.
///
/// `None` → `DEFAULT_OVERSAMPLE` (2×). `Some(v)` where `v < MIN_OVERSAMPLE`
/// is clamped up to `MIN_OVERSAMPLE` (1.0); unreasonably large values are
/// left as-is — the retry loop's `MAX_TOPK` cap is the hard ceiling on `k′`,
/// so a huge oversample just means we hit the cap in one iteration instead
/// of two.
pub(crate) fn resolve_oversample(raw: Option<f32>) -> f32 {
    match raw {
        None => DEFAULT_OVERSAMPLE,
        Some(v) => v.max(MIN_OVERSAMPLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_query_types::filter::FilterValue;

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
}
