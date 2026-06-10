use bytes::Bytes;
use shamir_tx::predicate_set::PredicateDep;
use shamir_types::core::interner::Interner;
use shamir_types::core::sort_codec;

use super::resolve::intern_field_path;
use crate::index::sorted_index_manager::SortedIndexManager;
use crate::query::filter::{Filter, FilterValue};

/// Encode a literal `FilterValue` into sort-codec bytes.
///
/// Returns `None` for non-literal / non-sortable variants.
pub(super) fn encode_filter_value(v: &FilterValue) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    match v {
        FilterValue::Bool(b) => {
            sort_codec::encode_bool(&mut buf, *b);
            Some(buf)
        }
        FilterValue::Int(i) => {
            sort_codec::encode_i64(&mut buf, *i);
            Some(buf)
        }
        FilterValue::Float(x) => {
            sort_codec::encode_f64(&mut buf, *x).ok()?;
            Some(buf)
        }
        FilterValue::String(s) => {
            sort_codec::encode_str(&mut buf, s);
            Some(buf)
        }
        FilterValue::Binary(b) => {
            sort_codec::encode_bytes(&mut buf, b);
            Some(buf)
        }
        _ => None,
    }
}

/// Build the physical lower bound key: `SORTED_TAG || name_interned || enc`.
pub(super) fn predicate_bound_lower(name_interned: u64, enc: &[u8]) -> std::ops::Bound<Bytes> {
    let mut k = Vec::with_capacity(9 + enc.len());
    k.push(shamir_tx::SORTED_TAG);
    k.extend_from_slice(&name_interned.to_be_bytes());
    k.extend_from_slice(enc);
    std::ops::Bound::Included(Bytes::from(k))
}

/// Build the physical upper bound key: `SORTED_TAG || name_interned || enc || 0xFF*16`.
pub(super) fn predicate_bound_upper(name_interned: u64, enc: &[u8]) -> std::ops::Bound<Bytes> {
    let mut k = Vec::with_capacity(9 + enc.len() + 16);
    k.push(shamir_tx::SORTED_TAG);
    k.extend_from_slice(&name_interned.to_be_bytes());
    k.extend_from_slice(enc);
    k.extend_from_slice(&[0xFFu8; 16]); // tiebreak pad (matches range_bounds :536-537)
    std::ops::Bound::Included(Bytes::from(k))
}

/// Build the physical prefix-only bound (start of index keyspace).
pub(super) fn predicate_bound_prefix(name_interned: u64) -> std::ops::Bound<Bytes> {
    let mut k = Vec::with_capacity(9);
    k.push(shamir_tx::SORTED_TAG);
    k.extend_from_slice(&name_interned.to_be_bytes());
    std::ops::Bound::Included(Bytes::from(k))
}

/// Build the full upper bound for the entire index: `SORTED_TAG || name_interned || 0xFF*64`.
pub(super) fn predicate_bound_full_upper(name_interned: u64) -> std::ops::Bound<Bytes> {
    let mut k = Vec::with_capacity(9 + 64);
    k.push(shamir_tx::SORTED_TAG);
    k.extend_from_slice(&name_interned.to_be_bytes());
    k.extend_from_slice(&[0xFFu8; 64]); // matches range_bounds :541-543
    std::ops::Bound::Included(Bytes::from(k))
}

/// Try to derive one `PredicateDep::IndexRange` from a single leaf filter.
///
/// Returns `true` if a mapping was emitted; `false` if the filter cannot be
/// mapped to a sorted-index interval (caller falls back to `TableScan`).
pub(super) fn predicate_handle_one(
    f: &Filter,
    sorted: &SortedIndexManager,
    interner: &Interner,
    table_token: u64,
    out: &mut smallvec::SmallVec<[PredicateDep; 2]>,
) -> bool {
    let (field, lo, hi): (
        &Vec<String>,
        std::ops::Bound<Vec<u8>>,
        std::ops::Bound<Vec<u8>>,
    ) = match f {
        Filter::Gt { field, value } | Filter::Gte { field, value } => {
            let enc = match encode_filter_value(value) {
                Some(e) => e,
                None => return false,
            };
            (
                field,
                std::ops::Bound::Included(enc),
                std::ops::Bound::Unbounded,
            )
        }
        Filter::Lt { field, value } | Filter::Lte { field, value } => {
            let enc = match encode_filter_value(value) {
                Some(e) => e,
                None => return false,
            };
            (
                field,
                std::ops::Bound::Unbounded,
                std::ops::Bound::Included(enc),
            )
        }
        Filter::Eq { field, value } | Filter::FieldEq { field, value } => {
            let enc = match encode_filter_value(value) {
                Some(e) => e,
                None => return false,
            };
            (
                field,
                std::ops::Bound::Included(enc.clone()),
                std::ops::Bound::Included(enc),
            )
        }
        Filter::Between { field, from, to } => {
            let lo = match encode_filter_value(from) {
                Some(e) => e,
                None => return false,
            };
            let hi = match encode_filter_value(to) {
                Some(e) => e,
                None => return false,
            };
            (
                field,
                std::ops::Bound::Included(lo),
                std::ops::Bound::Included(hi),
            )
        }
        _ => return false,
    };

    let path = match intern_field_path(field, interner) {
        Some(p) => p,
        None => return false,
    };
    let def = match sorted.find_by_field(&path) {
        Some(d) => d,
        None => return false,
    };
    let name = def.name_interned;

    let lo_b = match lo {
        std::ops::Bound::Included(e) => predicate_bound_lower(name, &e),
        std::ops::Bound::Unbounded => predicate_bound_prefix(name),
        std::ops::Bound::Excluded(_) => unreachable!(),
    };
    let hi_b = match hi {
        std::ops::Bound::Included(e) => predicate_bound_upper(name, &e),
        std::ops::Bound::Unbounded => predicate_bound_full_upper(name),
        std::ops::Bound::Excluded(_) => unreachable!(),
    };
    out.push(PredicateDep::IndexRange {
        table_token,
        index_id: name,
        lo: lo_b,
        hi: hi_b,
    });
    true
}

/// Derive zero or more `PredicateDep` from a `Filter` AST node.
///
/// Uses the table's sorted indexes to build precise byte-level intervals
/// where possible; returns an empty `SmallVec` when the filter cannot be
/// mapped (caller must fall back to a coarse `TableScan`).
///
/// For `And`: emits per-conjunct ranges for those that map; if ANY
/// conjunct fails to map, clears all precise ranges (safe over-lock:
/// the caller emits a single `TableScan` instead).
pub fn predicate_to_index_range(
    f: &Filter,
    sorted: &SortedIndexManager,
    interner: &Interner,
    table_token: u64,
) -> smallvec::SmallVec<[PredicateDep; 2]> {
    let mut out: smallvec::SmallVec<[PredicateDep; 2]> = smallvec::SmallVec::new();

    match f {
        Filter::And { filters } => {
            let mut all_mapped = true;
            for child in filters {
                if !predicate_handle_one(child, sorted, interner, table_token, &mut out) {
                    all_mapped = false;
                }
            }
            if !all_mapped {
                // Safe over-lock: drop precise parts and let caller emit TableScan.
                out.clear();
            }
        }
        // Coarse: cannot map to a precise index range.
        Filter::Or { .. }
        | Filter::Not { .. }
        | Filter::Regex { .. }
        | Filter::Like { .. }
        | Filter::ILike { .. }
        | Filter::Computed { .. }
        | Filter::Fts { .. }
        | Filter::VectorSimilarity { .. }
        | Filter::In { .. }
        | Filter::NotIn { .. }
        | Filter::Contains { .. }
        | Filter::ContainsAny { .. }
        | Filter::ContainsAll { .. }
        | Filter::IsNull { .. }
        | Filter::IsNotNull { .. }
        | Filter::Exists { .. }
        | Filter::NotExists { .. }
        | Filter::Ne { .. } => {
            // Return empty → caller records TableScan.
        }
        // Single leaf filter.
        other => {
            predicate_handle_one(other, sorted, interner, table_token, &mut out);
        }
    }
    out
}
