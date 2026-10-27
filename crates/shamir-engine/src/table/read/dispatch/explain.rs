use bytes::Bytes;

use crate::query::filter::eval::intern_field_path;
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{
    exec, CorruptRecordRef, ExplainPlan, PaginationInfo, PlanType, ReadQuery, SelectItem,
};
use crate::table::read::vector::filtered_vector::try_extract_filtered_vector_query;
use crate::table::table_manager::TableManager;

use shamir_storage::error::DbResult;
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

impl TableManager {
    /// Build an [`ExplainPlan`] by running only the planner decision tree
    /// (the same cascade as `read_impl`) but without materialising any rows.
    pub(in crate::table::read) fn build_explain_plan(
        &self,
        query: &ReadQuery,
        interner: &Interner,
    ) -> ExplainPlan {
        // 1. Counter shortcut: count(*) without WHERE.
        if query.r#where.is_none()
            && query.group_by.is_none()
            && query.order_by.is_none()
            && !query.select.distinct
            && !query.count_total
            && query.pagination.is_none()
            && query.select.items.len() == 1
        {
            if let SelectItem::CountAll { .. } = &query.select.items[0] {
                return ExplainPlan {
                    plan_type: PlanType::CounterShortcut,
                    index_used: Some("__record_counter__".into()),
                    estimated_rows: None,
                };
            }
            // MIN aggregate via sorted index.
            if let SelectItem::Aggregate {
                func: shamir_query_types::read::AggFunc::Min,
                field: shamir_query_types::read::AggregateField::Field(path),
                input: None,
                ..
            } = &query.select.items[0]
            {
                if let Some(fp) = intern_field_path(path, interner) {
                    if let Some(def) = self.sorted_indexes().find_by_field_ready(&fp) {
                        return ExplainPlan {
                            plan_type: PlanType::MinMaxIndex,
                            index_used: Some(format!("sorted_idx_{}_min", def.name_interned)),
                            estimated_rows: Some(1),
                        };
                    }
                }
            }
        }

        // 1b. V3.1: filtered ANN (And[VectorSimilarity, ...residual]).
        if let Some(ref filter) = query.r#where {
            if try_extract_filtered_vector_query(filter).is_some() {
                return ExplainPlan {
                    plan_type: PlanType::Index2,
                    index_used: Some("filtered_vector_scan".into()),
                    estimated_rows: None,
                };
            }
        }

        // 2. Index2 (FTS / Functional / Vector).
        if let Some(ref filter) = query.r#where {
            if !self.index2_registry().is_empty() {
                // Check FTS / Vector / Computed shapes without actually running lookup.
                let might_use_index2 = matches!(
                    filter,
                    crate::query::filter::Filter::Fts { .. }
                        | crate::query::filter::Filter::VectorSimilarity { .. }
                        | crate::query::filter::Filter::Computed { .. }
                );
                if might_use_index2 {
                    return ExplainPlan {
                        plan_type: PlanType::Index2,
                        index_used: Some("index2".into()),
                        estimated_rows: None,
                    };
                }
            }
        }

        // 3. BTree index scan (Eq / In / And).
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, _lookup_sets, _residual)) =
                self.try_plan_index_scan(filter, interner)
            {
                return ExplainPlan {
                    plan_type: PlanType::IndexScan,
                    index_used: Some(format!("idx_{idx_name}")),
                    estimated_rows: None,
                };
            }
        }

        // 3b. MAX aggregate via sorted index.
        if query.r#where.is_none()
            && query.group_by.is_none()
            && query.order_by.is_none()
            && !query.select.distinct
            && !query.count_total
            && query.pagination.is_none()
            && query.select.items.len() == 1
        {
            if let SelectItem::Aggregate {
                func: shamir_query_types::read::AggFunc::Max,
                field: shamir_query_types::read::AggregateField::Field(path),
                input: None,
                ..
            } = &query.select.items[0]
            {
                if let Some(fp) = intern_field_path(path, interner) {
                    if let Some(def) = self.sorted_indexes().find_by_field_ready(&fp) {
                        return ExplainPlan {
                            plan_type: PlanType::MinMaxIndex,
                            index_used: Some(format!("sorted_idx_{}_max", def.name_interned)),
                            estimated_rows: Some(1),
                        };
                    }
                }
            }
        }

        // 4. Keyset seek.
        if let Some((idx_name, _encoded_key, _after_id, _limit, _direction)) =
            self.try_plan_keyset_seek(query, interner)
        {
            return ExplainPlan {
                plan_type: PlanType::KeysetSeek,
                index_used: Some(format!("sorted_idx_{idx_name}")),
                estimated_rows: None,
            };
        }

        // 5. ORDER BY + LIMIT fast path.
        if let Some((idx_name, take, skip, _direction)) =
            self.try_plan_order_limit_fast_path(query, interner)
        {
            return ExplainPlan {
                plan_type: PlanType::OrderLimitFast,
                index_used: Some(format!("sorted_idx_{idx_name}")),
                estimated_rows: Some((skip + take) as u64),
            };
        }

        // 6. Sorted-index range scan (top-level).
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, _lo, _hi, _residual)) =
                self.try_plan_sorted_index_scan(filter, interner)
            {
                return ExplainPlan {
                    plan_type: PlanType::SortedIndexScan,
                    index_used: Some(format!("sorted_idx_{idx_name}")),
                    estimated_rows: None,
                };
            }
        }

        // 7. AND-range extraction.
        if let Some(ref filter) = query.r#where {
            if let Some((idx_name, _lo, _hi, _residual)) =
                self.try_plan_and_range_index_scan(filter, interner)
            {
                return ExplainPlan {
                    plan_type: PlanType::AndRangeIndexScan,
                    index_used: Some(format!("sorted_idx_{idx_name}")),
                    estimated_rows: None,
                };
            }
        }

        // 8. Full scan (fallback).
        ExplainPlan {
            plan_type: PlanType::FullScan,
            index_used: None,
            estimated_rows: None,
        }
    }
}

/// Opt #3a — LIMIT push-down for index scan paths.
///
/// When the query has **no in-memory ORDER BY, no GROUP BY, no DISTINCT,
/// no aggregates** (plain filtered SELECT with LIMIT [+ optional OFFSET]),
/// project ONLY the page rows instead of every matched row. Semantics are
/// byte-identical to the full-projection path: LIMIT without ORDER BY
/// already returns "the first N in scan order"; we just stop projecting
/// the rest.
///
/// Returns `None` (caller falls through to the full pipeline) when any
/// of the gate conditions fails or when there is no finite LIMIT to push.
/// `count_total` is preserved: the full match count equals `matched.len()`
/// — we can read it without projecting any row beyond the page.
pub(in crate::table::read) fn try_project_page_only(
    query: &ReadQuery,
    matched: &[(RecordId, InnerValue)],
    interner: &Interner,
    ctx: &FilterContext<'_>,
) -> DbResult<Option<(Vec<crate::query::read::QueryRecord>, Option<PaginationInfo>)>> {
    // Gate: every condition below disables the push-down.
    if query.order_by.is_some()
        || query.group_by.is_some()
        || query.select.distinct
        || exec::has_aggregates(&query.select)
    {
        return Ok(None);
    }

    // Need pagination or count_total — otherwise the original path is
    // already O(matches) and there's nothing to push down.
    if query.pagination.is_none() && !query.count_total {
        return Ok(None);
    }

    let (skip_u64, take_u64) = query.pagination.resolve();
    // Require a finite limit to project: without it, the page is the
    // whole tail and we'd still project every row. Skip-only optimisation
    // (no limit, offset > 0) would only save the prefix — not worth it
    // here; let the fall-through path handle it.
    let Some(take_u64) = take_u64 else {
        return Ok(None);
    };
    let take = take_u64 as usize;
    let skip = skip_u64 as usize;

    let total_matches = matched.len();
    let total_u64 = total_matches as u64;

    // Slice the page from `matched` before projection.
    let page_start = skip.min(total_matches);
    let page_end = skip.saturating_add(take).min(total_matches);
    let page_slice = &matched[page_start..page_end];

    let proj = exec::SelectProjection::new(&query.select, interner, ctx.scalars.clone())?
        .with_context(ctx);
    let mut paged: Vec<crate::query::read::QueryRecord> = Vec::with_capacity(page_slice.len());
    for (_, record) in page_slice {
        paged.push(crate::query::read::QueryRecord::Direct(
            proj.project_value(record, interner)?,
        ));
    }

    // Pagination metadata mirrors `apply_pagination`'s semantics: when
    // count_total is set, `total_count` is the full match count
    // (= matched.len()); otherwise we still get page_size / has_prev
    // from PaginationInfo::compute.
    let pagination = if query.pagination.is_none() && query.count_total {
        // count_total without pagination — same shape as apply_pagination.
        Some(PaginationInfo {
            total_count: Some(total_u64),
            total_pages: None,
            current_page: None,
            page_size: None,
            has_next: false,
            has_prev: false,
        })
    } else {
        let total_for_info = if query.count_total {
            Some(total_u64)
        } else {
            None
        };
        Some(PaginationInfo::compute(&query.pagination, total_for_info))
    };

    Ok(Some((paged, pagination)))
}

/// Byte-level twin of [`try_project_page_only`] — works on raw `Bytes`
/// instead of decoded `InnerValue`. Each row is wrapped in a zero-copy
/// `RecordView` for projection. Bare-scalar / non-map records where
/// `RecordView::new` fails are decoded via `InnerValue::from_bytes` as a
/// fallback (records are NEVER silently dropped).
///
/// F-30 (#823): `table` + `corrupt` let a row that fails BOTH decode
/// attempts be reported via the caller's `QueryResult::corrupt_records`
/// instead of silently vanishing (unchanged row-skip behaviour, only the
/// reporting changes — mirrors F-10/#800's convention in this same file).
pub(in crate::table::read) fn try_project_page_only_bytes(
    query: &ReadQuery,
    matched: &[(RecordId, Bytes)],
    interner: &Interner,
    ctx: &FilterContext<'_>,
    table: &str,
    corrupt: &mut Vec<CorruptRecordRef>,
) -> DbResult<Option<(Vec<crate::query::read::QueryRecord>, Option<PaginationInfo>)>> {
    // Same eligibility gate as the InnerValue variant.
    if query.order_by.is_some()
        || query.group_by.is_some()
        || query.select.distinct
        || exec::has_aggregates(&query.select)
    {
        return Ok(None);
    }
    if query.pagination.is_none() && !query.count_total {
        return Ok(None);
    }
    let (skip_u64, take_u64) = query.pagination.resolve();
    let Some(take_u64) = take_u64 else {
        return Ok(None);
    };
    let take = take_u64 as usize;
    let skip = skip_u64 as usize;

    let total_matches = matched.len();
    let total_u64 = total_matches as u64;

    let page_start = skip.min(total_matches);
    let page_end = skip.saturating_add(take).min(total_matches);
    let page_slice = &matched[page_start..page_end];

    let proj = exec::SelectProjection::new(&query.select, interner, ctx.scalars.clone())?
        .with_context(ctx);
    let mut paged: Vec<crate::query::read::QueryRecord> = Vec::with_capacity(page_slice.len());
    for (rid, bytes) in page_slice {
        let qv = match shamir_types::record_view::RecordView::new(bytes) {
            Ok(view) => proj.project_value(&view, interner)?,
            Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                Ok(iv) => proj.project_value(&iv, interner)?,
                Err(_) => {
                    corrupt.push(CorruptRecordRef {
                        table: table.to_string(),
                        id: *rid,
                    });
                    continue;
                }
            },
        };
        paged.push(crate::query::read::QueryRecord::Direct(qv));
    }

    let pagination = if query.pagination.is_none() && query.count_total {
        Some(PaginationInfo {
            total_count: Some(total_u64),
            total_pages: None,
            current_page: None,
            page_size: None,
            has_next: false,
            has_prev: false,
        })
    } else {
        let total_for_info = if query.count_total {
            Some(total_u64)
        } else {
            None
        };
        Some(PaginationInfo::compute(&query.pagination, total_for_info))
    };

    Ok(Some((paged, pagination)))
}

/// Byte-level twin of [`exec::apply_select_value`] — projects raw `Bytes`
/// rows via zero-copy `RecordView` instead of decoded `InnerValue`.
/// Bare-scalar / non-map records fall back to `InnerValue::from_bytes`.
///
/// F-26 (#819): fallible — `SelectProjection::new` rejects
/// `SelectItem::Expression`.
///
/// F-30 (#823): `table` + `corrupt` let a row that fails BOTH decode
/// attempts be reported via the caller's `QueryResult::corrupt_records`
/// instead of silently becoming a `QueryValue::Null` placeholder row
/// (unchanged row-skip-from-projection behaviour, only the reporting
/// changes — mirrors F-10/#800's convention in this same file).
pub(in crate::table::read) fn apply_select_value_bytes(
    matched: &[(RecordId, Bytes)],
    select: &crate::query::read::Select,
    interner: &Interner,
    ctx: &FilterContext<'_>,
    table: &str,
    corrupt: &mut Vec<CorruptRecordRef>,
) -> DbResult<Vec<QueryValue>> {
    let proj =
        exec::SelectProjection::new(select, interner, ctx.scalars.clone())?.with_context(ctx);
    matched
        .iter()
        .map(
            |(rid, bytes)| match shamir_types::record_view::RecordView::new(bytes) {
                Ok(view) => Ok(proj.project_value(&view, interner)?),
                Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                    Ok(iv) => Ok(proj.project_value(&iv, interner)?),
                    Err(_) => {
                        corrupt.push(CorruptRecordRef {
                            table: table.to_string(),
                            id: *rid,
                        });
                        Ok(QueryValue::Null)
                    }
                },
            },
        )
        .collect()
}

/// Vector predicates have no row-level full-scan representation.
pub(in crate::table::read) fn contains_vector_similarity(
    filter: &crate::query::filter::Filter,
) -> bool {
    match filter {
        crate::query::filter::Filter::VectorSimilarity { .. } => true,
        crate::query::filter::Filter::And { filters }
        | crate::query::filter::Filter::Or { filters } => {
            filters.iter().any(contains_vector_similarity)
        }
        crate::query::filter::Filter::Not { filter } => contains_vector_similarity(filter),
        _ => false,
    }
}
