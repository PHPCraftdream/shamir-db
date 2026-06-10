//! Index scan planning for TableManager.
//!
//! Contains all methods that decide *which* index to use for a read query:
//! equality / In / And index scans, sorted-index range planning, and the
//! ORDER BY + LIMIT K fast-path eligibility check.

use crate::query::filter::eval::{filter_value_to_inner, intern_field_path};
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::{exec, ReadQuery};
use shamir_types::core::interner::Interner;
use shamir_types::core::sort_codec;
use shamir_types::types::value::InnerValue;

use super::table_manager::TableManager;

impl TableManager {
    // ============================================================================
    // Index scan planning
    // ============================================================================

    pub(super) async fn try_plan_index2(
        &self,
        filter: &Filter,
        interner: &shamir_types::core::interner::Interner,
    ) -> Option<crate::index2::backend::IndexResult> {
        use crate::index2::backend::{FtsMode, IndexQuery};
        use crate::query::filter::eval::intern_field_path;

        if self.index2_registry().is_empty() {
            return None;
        }
        let registry = self.index2_registry();

        match filter {
            Filter::Fts { field, query, mode } => {
                let interned = intern_field_path(field, interner)?;
                let backend = registry.find_by_field_and_kind(&interned, "fts").await?;
                let tokens: Vec<u64> = backend.tokenize_query(query);
                let fts_mode = if mode == "or" {
                    FtsMode::OrAny
                } else {
                    FtsMode::AndAll
                };
                backend
                    .lookup(IndexQuery::Fts {
                        tokens,
                        mode: fts_mode,
                    })
                    .await
                    .ok()
            }
            Filter::VectorSimilarity { field, query, k } => {
                let interned = intern_field_path(field, interner)?;
                let backend = registry.find_by_field_and_kind(&interned, "vector").await?;
                backend
                    .lookup(IndexQuery::Vector {
                        vec: query.clone(),
                        k: *k,
                    })
                    .await
                    .ok()
            }
            Filter::Computed {
                field, cmp, value, ..
            } if cmp == "eq" => {
                let interned = intern_field_path(field, interner)?;
                let backend = registry
                    .find_by_field_and_kind(&interned, "functional")
                    .await?;
                let resolved = crate::query::filter::eval::filter_value_to_inner(value)?;
                let hash =
                    crate::index2::functional_backend::FunctionalBackend::hash_value(&resolved);
                backend
                    .lookup(IndexQuery::Point {
                        keys: smallvec::smallvec![hash.to_vec()],
                    })
                    .await
                    .ok()
            }
            _ => None,
        }
    }

    /// Try to find an index that can satisfy (part of) the filter.
    ///
    /// Returns `Some((index_name_interned, lookup_value_sets, residual_filter))`:
    /// - `lookup_value_sets` — one set per lookup (Eq -> 1 set, In -> N sets)
    /// - Each set is passed to `lookup_by_index` separately, results are unioned
    pub fn try_plan_index_scan(
        &self,
        filter: &Filter,
        interner: &Interner,
    ) -> Option<(u64, Vec<Vec<InnerValue>>, Option<Filter>)> {
        let idx_mgr = self.index_manager_ref();
        if !idx_mgr.has_indexes() {
            return None;
        }

        match filter {
            // Simple Eq: one lookup
            Filter::Eq { field, value } | Filter::FieldEq { field, value } => {
                let inner_val = filter_value_to_inner(value)?;
                let field_path = intern_field_path(field, interner)?;
                let idx = self.find_single_field_index(&field_path)?;
                Some((idx, vec![vec![inner_val]], None))
            }

            // In: multiple lookups, union results
            Filter::In { field, values } => {
                let field_path = intern_field_path(field, interner)?;
                let idx = self.find_single_field_index(&field_path)?;

                let mut sets = Vec::with_capacity(values.len());
                for v in values {
                    let inner = filter_value_to_inner(v)?;
                    sets.push(vec![inner]);
                }
                if sets.is_empty() {
                    return None;
                }
                Some((idx, sets, None))
            }

            // And: extract Eq/In conditions, try to match indexes
            Filter::And { filters } => self.try_plan_and_index_scan(filters, interner),

            _ => None,
        }
    }

    /// Find a single-field index whose path matches `field_path`.
    pub fn find_single_field_index(&self, field_path: &[u64]) -> Option<u64> {
        for def in self.index_manager_ref().iter_indexes() {
            if def.paths.len() == 1 && def.paths[0].path == field_path {
                return Some(def.name_interned);
            }
        }
        None
    }

    /// Try to plan an index scan from an And filter.
    fn try_plan_and_index_scan(
        &self,
        filters: &[Filter],
        interner: &Interner,
    ) -> Option<(u64, Vec<Vec<InnerValue>>, Option<Filter>)> {
        // Collect indexable conditions: (filter_index, field_path, lookup_sets)
        // Eq -> 1 set, In -> N sets
        struct IndexableItem {
            filter_idx: usize,
            field_path: Vec<u64>,
            lookup_sets: Vec<Vec<InnerValue>>,
        }

        let mut items: Vec<IndexableItem> = Vec::new();
        for (i, f) in filters.iter().enumerate() {
            match f {
                Filter::Eq { field, value } | Filter::FieldEq { field, value } => {
                    if let Some(inner) = filter_value_to_inner(value) {
                        if let Some(fp) = intern_field_path(field, interner) {
                            items.push(IndexableItem {
                                filter_idx: i,
                                field_path: fp,
                                lookup_sets: vec![vec![inner]],
                            });
                        }
                    }
                }
                Filter::In { field, values } => {
                    if let Some(fp) = intern_field_path(field, interner) {
                        let mut sets = Vec::new();
                        let mut all_literal = true;
                        for v in values {
                            if let Some(inner) = filter_value_to_inner(v) {
                                sets.push(vec![inner]);
                            } else {
                                all_literal = false;
                                break;
                            }
                        }
                        if all_literal && !sets.is_empty() {
                            items.push(IndexableItem {
                                filter_idx: i,
                                field_path: fp,
                                lookup_sets: sets,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        if items.is_empty() {
            return None;
        }

        let idx_mgr = self.index_manager_ref();

        // Try composite indexes first (Eq-only, each path covered by exactly one Eq)
        for def in idx_mgr.iter_indexes() {
            if def.paths.len() > 1 {
                let mut lookup_values = Vec::with_capacity(def.paths.len());
                let mut consumed = Vec::new();
                let mut all_matched = true;

                for idx_path in &def.paths {
                    if let Some(item) = items
                        .iter()
                        .find(|it| it.field_path == idx_path.path && it.lookup_sets.len() == 1)
                    {
                        lookup_values.push(item.lookup_sets[0][0].clone());
                        consumed.push(item.filter_idx);
                    } else {
                        all_matched = false;
                        break;
                    }
                }

                if all_matched {
                    let residual = Self::build_residual(filters, &consumed);
                    return Some((def.name_interned, vec![lookup_values], residual));
                }
            }
        }

        // Try single-field indexes (Eq or In)
        for def in idx_mgr.iter_indexes() {
            if def.paths.len() == 1 {
                if let Some(item) = items.iter().find(|it| it.field_path == def.paths[0].path) {
                    let consumed = vec![item.filter_idx];
                    let residual = Self::build_residual(filters, &consumed);
                    return Some((def.name_interned, item.lookup_sets.clone(), residual));
                }
            }
        }

        None
    }

    /// Build residual filter from And children, excluding consumed indices.
    fn build_residual(filters: &[Filter], consumed: &[usize]) -> Option<Filter> {
        let remaining: Vec<Filter> = filters
            .iter()
            .enumerate()
            .filter(|(i, _)| !consumed.contains(i))
            .map(|(_, f)| f.clone())
            .collect();

        match remaining.len() {
            0 => None,
            1 => Some(remaining.into_iter().next().unwrap()),
            _ => Some(Filter::And { filters: remaining }),
        }
    }

    /// Try to plan a sorted-index scan for the supported range
    /// filters (`Between`, `Gte`, `Lte`). Returns
    /// `(name_interned, lower_encoded, upper_encoded, residual)` or
    /// `None` if no sorted index applies.
    ///
    /// `lower_encoded` / `upper_encoded` are bytes produced by
    /// `sort_codec` — `None` for an open bound.
    ///
    /// Gt / Lt are intentionally NOT routed here yet — they need an
    /// "exclude exact-match boundary" trick that we'll add in a
    /// follow-up. They fall through to the full-scan path.
    #[allow(clippy::type_complexity)] // scan plan tuple; kept unpacked for caller convenience
    pub fn try_plan_sorted_index_scan(
        &self,
        filter: &Filter,
        interner: &Interner,
    ) -> Option<(u64, Option<Vec<u8>>, Option<Vec<u8>>, Option<Filter>)> {
        let mgr = self.sorted_indexes();
        if !mgr.has_indexes() {
            return None;
        }

        match filter {
            Filter::Between { field, from, to } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let lo = encode_filter_value_for_sort(from)?;
                let hi = encode_filter_value_for_sort(to)?;
                Some((def.name_interned, Some(lo), Some(hi), None))
            }
            Filter::Gte { field, value } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let lo = encode_filter_value_for_sort(value)?;
                Some((def.name_interned, Some(lo), None, None))
            }
            Filter::Lte { field, value } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let hi = encode_filter_value_for_sort(value)?;
                Some((def.name_interned, None, Some(hi), None))
            }
            // Q2: strict-bound variants. We don't try to compute an
            // exclusive byte-suffix successor (encoding-dependent,
            // brittle); instead we use the inclusive Gte/Lte index
            // window and add an `Ne(value)` residual filter to
            // exclude the boundary at evaluation time. Cheap — the
            // boundary value typically yields at most a handful of
            // records to filter.
            Filter::Gt { field, value } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let lo = encode_filter_value_for_sort(value)?;
                let residual = Filter::Ne {
                    field: field.clone(),
                    value: value.clone(),
                };
                Some((def.name_interned, Some(lo), None, Some(residual)))
            }
            Filter::Lt { field, value } => {
                let field_path = intern_field_path(field, interner)?;
                let def = mgr.find_by_field(&field_path)?;
                let hi = encode_filter_value_for_sort(value)?;
                let residual = Filter::Ne {
                    field: field.clone(),
                    value: value.clone(),
                };
                Some((def.name_interned, None, Some(hi), Some(residual)))
            }
            _ => None,
        }
    }

    /// Eligibility check for the ORDER BY + LIMIT K fast path
    /// (both ASC and DESC).
    ///
    /// Returns `Some((sorted_index_name, take, skip, direction))`
    /// when the query is:
    ///
    /// - `order_by: { items: [single_item] }` (either direction)
    /// - `pagination: LimitOffset` with a finite `limit` (paged form
    ///   normalised through `pagination.resolve()`)
    /// - no `where`, `group_by`, `select.distinct`, `count_total`,
    ///   no aggregate items in `select`
    /// - a sorted index covers the order_by field
    ///
    /// The fast path materialises only `skip + take` index entries
    /// in the requested order (asc via `lookup_first_k`, desc via
    /// `lookup_last_k`), then projects.
    pub fn try_plan_order_limit_fast_path(
        &self,
        query: &ReadQuery,
        interner: &Interner,
    ) -> Option<(u64, usize, usize, shamir_query_types::read::OrderDirection)> {
        // Shape guards.
        if query.r#where.is_some()
            || query.group_by.is_some()
            || query.select.distinct
            || query.count_total
            || exec::has_aggregates(&query.select)
        {
            return None;
        }
        let order_by = query.order_by.as_ref()?;
        if order_by.items.len() != 1 {
            return None;
        }
        let item = &order_by.items[0];
        // Pagination must yield a finite take.
        let (skip, take_opt) = query.pagination.resolve();
        let take = take_opt? as usize;
        if take == 0 {
            return None;
        }
        let skip = skip as usize;

        // Sorted index must cover the order_by field.
        let mgr = self.sorted_indexes();
        if !mgr.has_indexes() {
            return None;
        }
        let field_path = intern_field_path(&item.field, interner)?;
        let def = mgr.find_by_field(&field_path)?;

        Some((def.name_interned, take, skip, item.direction))
    }
}

/// Encode a scalar `FilterValue` with `sort_codec` so range bounds
/// can be compared to physical sorted-index keys. Returns `None` for
/// values that can't be indexed (NaN floats, non-scalars).
pub(super) fn encode_filter_value_for_sort(value: &FilterValue) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    match value {
        FilterValue::Int(i) => sort_codec::encode_i64(&mut buf, *i),
        FilterValue::Float(f) => sort_codec::encode_f64(&mut buf, *f).ok()?,
        FilterValue::String(s) => sort_codec::encode_str(&mut buf, s),
        FilterValue::Bool(b) => sort_codec::encode_bool(&mut buf, *b),
        FilterValue::Null => sort_codec::encode_null(&mut buf),
        FilterValue::Binary(b) => sort_codec::encode_bytes(&mut buf, b),
        _ => return None,
    }
    Some(buf)
}
