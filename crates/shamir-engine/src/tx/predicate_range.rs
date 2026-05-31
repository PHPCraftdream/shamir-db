//! Filter → `PredicateDep::IndexRange` bridge.
//!
//! Phase C, step 2b. Lives in the engine because it speaks `Filter`
//! (shamir-query-types) + `SortedIndexManager` + `Interner`
//! (shamir-engine). Wired into `crate::tx::mod`.
//!
//! Zero-overhead on Snapshot / non-tx: this module is never called
//! unless the caller has already gated on
//! `isolation == IsolationLevel::Serializable`.

use bytes::Bytes;
use std::ops::Bound;

use crate::index::sorted_index_manager::SortedIndexManager;
use crate::query::filter::eval::intern_field_path;
use crate::query::filter::{Filter, FilterValue};
use shamir_tx::predicate_set::{PredicateDep, SORTED_PREFIX_LEN, SORTED_TAG};
use shamir_types::core::interner::Interner;
use shamir_types::core::sort_codec;

/// 16 bytes of 0xFF — matches `range_bounds` upper-tiebreaker
/// (`sorted_index_manager.rs:537`). Sized so every possible 16-byte
/// `RecordId` suffix at the same encoded value compares <= this bound.
const RID_MAX: [u8; 16] = [0xFFu8; 16];

/// 64 bytes of 0xFF — matches `range_bounds(_, None)` open-upper
/// (`sorted_index_manager.rs:543`). Bigger than any real
/// encoded_value + rid suffix in one index's keyspace.
const OPEN_UPPER_TAIL: [u8; 64] = [0xFFu8; 64];

/// Build the 9-byte sorted-index prefix. Mirror of the private
/// `SortedIndexManager::entry_prefix` (:574).
#[inline]
fn entry_prefix(name_interned: u64) -> [u8; SORTED_PREFIX_LEN] {
    let mut p = [0u8; SORTED_PREFIX_LEN];
    p[0] = SORTED_TAG;
    p[1..].copy_from_slice(&name_interned.to_be_bytes());
    p
}

/// Encode one literal `FilterValue` through `sort_codec`, producing
/// the same bytes `extract_and_encode` would for the matching
/// `InnerValue` (`sorted_index_manager.rs:638-647`). Returns `None`
/// for non-literal or non-encodable values (`FieldRef`, `QueryRef`,
/// `Array`, NaN, ...).
fn encode_filter_value(v: &FilterValue) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    match v {
        FilterValue::Null => sort_codec::encode_null(&mut buf),
        FilterValue::Bool(b) => sort_codec::encode_bool(&mut buf, *b),
        FilterValue::Int(i) => sort_codec::encode_i64(&mut buf, *i),
        FilterValue::Float(f) => sort_codec::encode_f64(&mut buf, *f).ok()?,
        FilterValue::String(s) => sort_codec::encode_str(&mut buf, s),
        FilterValue::Binary(b) => sort_codec::encode_bytes(&mut buf, b),
        // FieldRef/QueryRef/FnCall/Expr/Cond/Array -> no static byte
        // form; caller falls back to TableScan.
        _ => return None,
    }
    Some(buf)
}

// ---------------------------------------------------------------------------
// Bound constructors
// ---------------------------------------------------------------------------

#[inline]
fn make_bound_lo_incl(prefix: &[u8], enc: &[u8]) -> Bound<Bytes> {
    let mut k = Vec::with_capacity(prefix.len() + enc.len());
    k.extend_from_slice(prefix);
    k.extend_from_slice(enc);
    Bound::Included(Bytes::from(k))
}

#[inline]
fn make_bound_lo_excl_after(prefix: &[u8], enc: &[u8]) -> Bound<Bytes> {
    // "Strictly after value v" — past every rid tiebreaker for v.
    let mut k = Vec::with_capacity(prefix.len() + enc.len() + RID_MAX.len());
    k.extend_from_slice(prefix);
    k.extend_from_slice(enc);
    k.extend_from_slice(&RID_MAX);
    Bound::Excluded(Bytes::from(k))
}

#[inline]
fn make_bound_hi_excl(prefix: &[u8], enc: &[u8]) -> Bound<Bytes> {
    let mut k = Vec::with_capacity(prefix.len() + enc.len());
    k.extend_from_slice(prefix);
    k.extend_from_slice(enc);
    Bound::Excluded(Bytes::from(k))
}

#[inline]
fn make_bound_hi_incl_value_max(prefix: &[u8], enc: &[u8]) -> Bound<Bytes> {
    // "Up to and including value v" — all rids at v.
    let mut k = Vec::with_capacity(prefix.len() + enc.len() + RID_MAX.len());
    k.extend_from_slice(prefix);
    k.extend_from_slice(enc);
    k.extend_from_slice(&RID_MAX);
    Bound::Included(Bytes::from(k))
}

#[inline]
fn make_bound_lo_prefix(prefix: &[u8]) -> Bound<Bytes> {
    Bound::Included(Bytes::copy_from_slice(prefix))
}

#[inline]
fn make_bound_hi_open(prefix: &[u8]) -> Bound<Bytes> {
    let mut k = Vec::with_capacity(prefix.len() + OPEN_UPPER_TAIL.len());
    k.extend_from_slice(prefix);
    k.extend_from_slice(&OPEN_UPPER_TAIL);
    Bound::Included(Bytes::from(k))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Map ONE scalar `Filter` to a precise `IndexRange` `PredicateDep` iff:
///   (a) the operator is order-comparable (Eq/Gt/Gte/Lt/Lte/Between),
///   (b) the field path interns successfully,
///   (c) a sorted index covers that field (`SortedIndexManager::find_by_field`),
///   (d) the value encodes via `sort_codec`.
///
/// Returns `None` for `And`/`Or`/`Not`/`Regex`/`Like`/`ILike`/`Computed`/
/// `Fts`/`VectorSimilarity`/`In`/`NotIn`/`Contains*`/`IsNull`/`IsNotNull`/
/// `Exists`/`NotExists`/`FieldEq`/`Ne` — callers record `TableScan`.
///
/// For `And` use [`predicate_to_index_deps`] (handles each conjunct
/// independently; union of per-conjunct ranges is sound — doc section 3.2 /
/// section 7.7).
///
/// `Ne` coarsens to `None` because `field != v` is `Lt(v) | Gt(v)` — two
/// disjoint intervals; emitting one over-locks the universe (equivalent
/// to `TableScan`). Future optimization: emit both intervals.
pub fn predicate_to_index_range(
    f: &Filter,
    sorted: &SortedIndexManager,
    interner: &Interner,
    table_token: u64,
) -> Option<PredicateDep> {
    let lookup = |field: &[String]| -> Option<(u64, [u8; SORTED_PREFIX_LEN])> {
        let path = intern_field_path(field, interner)?;
        let def = sorted.find_by_field(&path)?;
        let prefix = entry_prefix(def.name_interned);
        Some((def.name_interned, prefix))
    };

    match f {
        Filter::Eq { field, value } => {
            let (idx, prefix) = lookup(field)?;
            let enc = encode_filter_value(value)?;
            Some(PredicateDep::IndexRange {
                table_token,
                index_id: idx,
                lo: make_bound_lo_incl(&prefix, &enc),
                hi: make_bound_hi_incl_value_max(&prefix, &enc),
            })
        }
        Filter::Gt { field, value } => {
            let (idx, prefix) = lookup(field)?;
            let enc = encode_filter_value(value)?;
            Some(PredicateDep::IndexRange {
                table_token,
                index_id: idx,
                lo: make_bound_lo_excl_after(&prefix, &enc),
                hi: make_bound_hi_open(&prefix),
            })
        }
        Filter::Gte { field, value } => {
            let (idx, prefix) = lookup(field)?;
            let enc = encode_filter_value(value)?;
            Some(PredicateDep::IndexRange {
                table_token,
                index_id: idx,
                lo: make_bound_lo_incl(&prefix, &enc),
                hi: make_bound_hi_open(&prefix),
            })
        }
        Filter::Lt { field, value } => {
            let (idx, prefix) = lookup(field)?;
            let enc = encode_filter_value(value)?;
            Some(PredicateDep::IndexRange {
                table_token,
                index_id: idx,
                lo: make_bound_lo_prefix(&prefix),
                hi: make_bound_hi_excl(&prefix, &enc),
            })
        }
        Filter::Lte { field, value } => {
            let (idx, prefix) = lookup(field)?;
            let enc = encode_filter_value(value)?;
            Some(PredicateDep::IndexRange {
                table_token,
                index_id: idx,
                lo: make_bound_lo_prefix(&prefix),
                hi: make_bound_hi_incl_value_max(&prefix, &enc),
            })
        }
        Filter::Between { field, from, to } => {
            let (idx, prefix) = lookup(field)?;
            let lo_enc = encode_filter_value(from)?;
            let hi_enc = encode_filter_value(to)?;
            Some(PredicateDep::IndexRange {
                table_token,
                index_id: idx,
                lo: make_bound_lo_incl(&prefix, &lo_enc),
                hi: make_bound_hi_incl_value_max(&prefix, &hi_enc),
            })
        }
        // And/Or/Not/Regex/Like/ILike/Computed/Fts/VectorSimilarity/
        // In/NotIn/Contains*/IsNull/IsNotNull/Exists/NotExists/
        // FieldEq/Ne => None.
        //
        // FieldEq is intentionally NOT routed to Eq here: it carries
        // the same shape but historically marks "shortcut field=value".
        // Flip to Eq path if bench shows the precise path is worth it.
        _ => None,
    }
}

/// Recursive walk: one `PredicateDep` per scalar `Filter`, flattening
/// `And` conjuncts. `Or`/`Not` (and any non-mappable scalar) signal
/// "give up on precise" by inserting NOTHING — caller must record a
/// coarse `PredicateDep::TableScan { table_token }` per doc section 3.2
/// row 5/7.
///
/// Returns `(deps, all_precise)`. When `all_precise == false` the caller
/// must additionally append a `TableScan` (the deps alone are not safe
/// to cover the full predicate).
pub fn predicate_to_index_deps(
    f: &Filter,
    sorted: &SortedIndexManager,
    interner: &Interner,
    table_token: u64,
) -> (Vec<PredicateDep>, bool) {
    let mut out = Vec::new();
    let precise = walk(f, sorted, interner, table_token, &mut out);
    (out, precise)
}

fn walk(
    f: &Filter,
    sorted: &SortedIndexManager,
    interner: &Interner,
    table_token: u64,
    out: &mut Vec<PredicateDep>,
) -> bool {
    match f {
        Filter::And { filters } => {
            // Per-conjunct dep: union over-locks safely (doc section 7
            // risk 7). ALL conjuncts must be precise for the And to be
            // precise; if even one falls back, we have to over-lock
            // anyway -> coarse.
            let mut all = true;
            for sub in filters {
                if !walk(sub, sorted, interner, table_token, out) {
                    all = false;
                }
            }
            all
        }
        // Or/Not -> coarse: a precise per-disjunct dep is unsound for
        // Or (we'd need ALL disjuncts to be present, not the union).
        Filter::Or { .. } | Filter::Not { .. } => false,
        // Scalar form.
        _ => match predicate_to_index_range(f, sorted, interner, table_token) {
            Some(d) => {
                out.push(d);
                true
            }
            None => false,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests — inline (engine crate convention: tx/tests/ dir is for
// end-to-end SSI integration; pure unit tests for a helper module live
// next to it).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;
    use shamir_tx::predicate_set::key_in_interval;
    use shamir_types::core::interner::{Interner, TouchInd};
    use std::sync::Arc;

    use crate::index::sorted_index_manager::SortedIndexDefinition;

    // ---- fixture ---------------------------------------------------------

    async fn fixture() -> (
        SortedIndexManager,
        Arc<Interner>,
        u64, /* idx_name_id */
    ) {
        let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let mgr = SortedIndexManager::new(Arc::clone(&info_store))
            .await
            .unwrap();

        let interner = Arc::new(Interner::new());
        let age_id = match interner.touch_ind("age").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };

        let idx_name_id = match interner.touch_ind("by_age").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        mgr.register(SortedIndexDefinition::new(idx_name_id, vec![age_id]))
            .await
            .unwrap();
        (mgr, interner, idx_name_id)
    }

    // ---- mapping tests ---------------------------------------------------

    #[tokio::test]
    async fn maps_gt_to_excluded_lo() {
        let (mgr, interner, idx_id) = fixture().await;
        let f = Filter::Gt {
            field: vec!["age".into()],
            value: FilterValue::Int(30),
        };
        let dep = predicate_to_index_range(&f, &mgr, &interner, 42).unwrap();
        match dep {
            PredicateDep::IndexRange {
                table_token,
                index_id,
                lo,
                hi,
            } => {
                assert_eq!(table_token, 42);
                assert_eq!(index_id, idx_id);
                assert!(matches!(lo, Bound::Excluded(_)));
                assert!(matches!(hi, Bound::Included(_)));
            }
            _ => panic!("expected IndexRange"),
        }
    }

    #[tokio::test]
    async fn maps_gte_to_included_lo() {
        let (mgr, interner, _idx_id) = fixture().await;
        let f = Filter::Gte {
            field: vec!["age".into()],
            value: FilterValue::Int(30),
        };
        let dep = predicate_to_index_range(&f, &mgr, &interner, 1).unwrap();
        if let PredicateDep::IndexRange { lo, .. } = dep {
            assert!(matches!(lo, Bound::Included(_)));
        } else {
            panic!("expected IndexRange");
        }
    }

    #[tokio::test]
    async fn maps_lt_lte_between_eq() {
        let (mgr, interner, _idx) = fixture().await;
        for f in [
            Filter::Lt {
                field: vec!["age".into()],
                value: FilterValue::Int(30),
            },
            Filter::Lte {
                field: vec!["age".into()],
                value: FilterValue::Int(30),
            },
            Filter::Between {
                field: vec!["age".into()],
                from: FilterValue::Int(10),
                to: FilterValue::Int(20),
            },
            Filter::Eq {
                field: vec!["age".into()],
                value: FilterValue::Int(30),
            },
        ] {
            assert!(
                predicate_to_index_range(&f, &mgr, &interner, 1).is_some(),
                "{:?}",
                f
            );
        }
    }

    #[tokio::test]
    async fn returns_none_for_unsupported_ops() {
        let (mgr, interner, _idx) = fixture().await;
        for f in [
            Filter::Ne {
                field: vec!["age".into()],
                value: FilterValue::Int(0),
            },
            Filter::Like {
                field: vec!["age".into()],
                pattern: "x%".into(),
            },
            Filter::ILike {
                field: vec!["age".into()],
                pattern: "x%".into(),
            },
            Filter::Regex {
                field: vec!["age".into()],
                pattern: "^x".into(),
            },
            Filter::Computed {
                expr_op: "lower".into(),
                field: vec!["age".into()],
                expr_args: None,
                cmp: "eq".into(),
                value: FilterValue::Int(0),
            },
            Filter::Fts {
                field: vec!["age".into()],
                query: "x".into(),
                mode: "and".into(),
            },
            Filter::IsNull {
                field: vec!["age".into()],
            },
            Filter::In {
                field: vec!["age".into()],
                values: vec![FilterValue::Int(1)],
            },
            Filter::Or { filters: vec![] },
            Filter::Not {
                filter: Box::new(Filter::Eq {
                    field: vec!["age".into()],
                    value: FilterValue::Int(0),
                }),
            },
        ] {
            assert!(
                predicate_to_index_range(&f, &mgr, &interner, 1).is_none(),
                "should be coarse: {:?}",
                f
            );
        }
    }

    #[tokio::test]
    async fn returns_none_when_no_index_on_field() {
        let (mgr, interner, _idx) = fixture().await;
        let f = Filter::Gt {
            field: vec!["weight".into()],
            value: FilterValue::Int(30),
        };
        assert!(predicate_to_index_range(&f, &mgr, &interner, 1).is_none());
    }

    #[tokio::test]
    async fn returns_none_for_unencodable_value() {
        let (mgr, interner, _idx) = fixture().await;
        let f = Filter::Eq {
            field: vec!["age".into()],
            value: FilterValue::field_ref("other"),
        };
        assert!(predicate_to_index_range(&f, &mgr, &interner, 1).is_none());
        let nan = Filter::Gt {
            field: vec!["age".into()],
            value: FilterValue::Float(f64::NAN),
        };
        assert!(predicate_to_index_range(&nan, &mgr, &interner, 1).is_none());
    }

    // ---- And handling ----------------------------------------------------

    #[tokio::test]
    async fn and_emits_one_dep_per_scalar_conjunct() {
        let (mgr, interner, _idx) = fixture().await;
        let f = Filter::And {
            filters: vec![
                Filter::Gt {
                    field: vec!["age".into()],
                    value: FilterValue::Int(10),
                },
                Filter::Lt {
                    field: vec!["age".into()],
                    value: FilterValue::Int(20),
                },
            ],
        };
        let (deps, precise) = predicate_to_index_deps(&f, &mgr, &interner, 1);
        assert_eq!(deps.len(), 2);
        assert!(precise);
    }

    #[tokio::test]
    async fn and_with_one_coarse_conjunct_marks_overall_coarse() {
        let (mgr, interner, _idx) = fixture().await;
        let f = Filter::And {
            filters: vec![
                Filter::Gt {
                    field: vec!["age".into()],
                    value: FilterValue::Int(10),
                },
                Filter::Regex {
                    field: vec!["age".into()],
                    pattern: "x".into(),
                },
            ],
        };
        let (deps, precise) = predicate_to_index_deps(&f, &mgr, &interner, 1);
        assert_eq!(deps.len(), 1);
        assert!(!precise);
    }

    // ---- Round-trip with key_in_interval ---------------------------------

    #[tokio::test]
    async fn gt_dep_round_trips_through_key_in_interval() {
        let (mgr, interner, idx_id) = fixture().await;
        let dep = predicate_to_index_range(
            &Filter::Gt {
                field: vec!["age".into()],
                value: FilterValue::Int(30),
            },
            &mgr,
            &interner,
            42,
        )
        .unwrap();
        let (lo, hi) = match &dep {
            PredicateDep::IndexRange { lo, hi, .. } => (lo, hi),
            _ => panic!("expected IndexRange"),
        };

        let make_posting = |v: i64, rid: u8| {
            let mut e = Vec::new();
            sort_codec::encode_i64(&mut e, v);
            let mut k = vec![SORTED_TAG];
            k.extend_from_slice(&idx_id.to_be_bytes());
            k.extend_from_slice(&e);
            k.extend_from_slice(&[rid; 16]);
            k
        };

        // age=31 -> in
        assert!(key_in_interval(&make_posting(31, 0), idx_id, lo, hi));
        // age=30 — boundary, Gt is strict => out (any rid)
        for rid in [0u8, 0x7F, 0xFF] {
            assert!(
                !key_in_interval(&make_posting(30, rid), idx_id, lo, hi),
                "rid {rid:#x}"
            );
        }
        // age=29 -> out
        assert!(!key_in_interval(&make_posting(29, 0xFF), idx_id, lo, hi));
    }

    #[tokio::test]
    async fn between_dep_round_trips_through_key_in_interval() {
        let (mgr, interner, idx_id) = fixture().await;
        let dep = predicate_to_index_range(
            &Filter::Between {
                field: vec!["age".into()],
                from: FilterValue::Int(10),
                to: FilterValue::Int(20),
            },
            &mgr,
            &interner,
            42,
        )
        .unwrap();
        let (lo, hi) = match &dep {
            PredicateDep::IndexRange { lo, hi, .. } => (lo, hi),
            _ => panic!("expected IndexRange"),
        };

        let make_posting = |v: i64, rid: u8| {
            let mut e = Vec::new();
            sort_codec::encode_i64(&mut e, v);
            let mut k = vec![SORTED_TAG];
            k.extend_from_slice(&idx_id.to_be_bytes());
            k.extend_from_slice(&e);
            k.extend_from_slice(&[rid; 16]);
            k
        };

        for v in [10, 15, 20] {
            assert!(
                key_in_interval(&make_posting(v, 0x42), idx_id, lo, hi),
                "{v} in [10,20]"
            );
        }
        for v in [9, 21] {
            assert!(
                !key_in_interval(&make_posting(v, 0x42), idx_id, lo, hi),
                "{v} NOT in [10,20]"
            );
        }
    }
}
