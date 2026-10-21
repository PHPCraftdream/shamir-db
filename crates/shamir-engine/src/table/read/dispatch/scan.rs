use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;

use futures::StreamExt;

use crate::query::filter::eval::{compile_filter, intern_field_path, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{exec, CorruptRecordRef, QueryRecord, QueryResult, QueryStats, ReadQuery};
use crate::table::table_manager::TableManager;

use shamir_query_types::batch::ResultEncoding;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::table::read::dispatch::explain::apply_select_value_bytes;
use crate::table::read::execution::dispatch::collect_versions;
use crate::table::read::planning::read_planner::encode_query_value_for_sort;

impl TableManager {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::table::read) async fn read_scan(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        tx: Option<&shamir_tx::TxContext>,
        encoding: ResultEncoding,
        batch_size: usize,
        start: Instant,
    ) -> DbResult<QueryResult> {
        // Fall back to full scan
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let has_order = query.order_by.is_some();
        let has_distinct = query.select.distinct;

        let filter_cb: Option<Arc<FilterNode>> = query
            .r#where
            .as_ref()
            .map(|f| Arc::new(compile_filter(f, interner)));

        let needs_full_collect = has_group_by || has_agg || has_order || has_distinct;

        if needs_full_collect {
            // S-read fallback: ORDER BY / GROUP BY / DISTINCT / aggregates
            // require in-memory collection and QueryValue-based post-processing.
            // Id encoding is not applicable here — fall back to Name.
            self.read_collecting(
                query,
                ctx,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                batch_size,
                start,
                tx,
            )
            .await
        } else if query.count_total {
            self.read_counting(
                query,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                ctx,
                batch_size,
                start,
                tx,
            )
            .await
        } else {
            self.read_streaming(
                query,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                ctx,
                batch_size,
                start,
                tx,
                encoding,
            )
            .await
        }
    }

    /// Compare snapshot rows before projection: skip/take alone loses the cursor.
    /// Like collecting ORDER BY, results use Name encoding.
    pub(in crate::table::read) async fn read_snapshot_keyset(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        tx: Option<&shamir_tx::TxContext>,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let order = query.order_by.as_ref().ok_or_else(|| {
            DbError::Validation("keyset pagination requires ORDER BY".to_string())
        })?;
        let keyset = query.pagination.keyset();
        let key = keyset.map(|(key, _)| key);
        if key.is_some_and(|key| key.len() != order.items.len() || key.is_empty())
            || order.items.is_empty()
            || query.group_by.is_some()
            || exec::has_aggregates(&query.select)
            || query.select.distinct
        {
            return Err(DbError::Validation(
                "snapshot keyset requires matching ORDER BY keys and non-collapsing SELECT"
                    .to_string(),
            ));
        }
        let seek: Vec<Vec<u8>> = key
            .into_iter()
            .flatten()
            .zip(&order.items)
            .map(|(value, item)| {
                if item.field.len() == 1 && item.field[0] == "_id" {
                    let id = match value {
                        QueryValue::Str(value) => value.parse::<RecordId>().ok(),
                        QueryValue::Bin(value) => RecordId::try_from_bytes(value),
                        _ => None,
                    }
                    .ok_or_else(|| {
                        DbError::Validation("_id keyset cursor must be a valid RecordId".into())
                    })?;
                    return Ok(id.as_bytes().to_vec());
                }
                encode_query_value_for_sort(value).ok_or_else(|| {
                    DbError::Validation("keyset values must be indexable scalars".to_string())
                })
            })
            .collect::<DbResult<_>>()?;
        let paths: Vec<_> = order
            .items
            .iter()
            .map(|item| intern_field_path(&item.field, interner))
            .collect();
        let compare = |left: &[Vec<u8>], right: &[Vec<u8>]| {
            for ((a, b), item) in left.iter().zip(right).zip(&order.items) {
                let cmp = a.cmp(b);
                let cmp = if item.direction == shamir_query_types::read::OrderDirection::Desc {
                    cmp.reverse()
                } else {
                    cmp
                };
                if !cmp.is_eq() {
                    return cmp;
                }
            }
            std::cmp::Ordering::Equal
        };
        let compare_ids = |a: &RecordId, b: &RecordId| {
            if order.items.last().expect("nonempty order").direction
                == shamir_query_types::read::OrderDirection::Desc
            {
                b.cmp(a)
            } else {
                a.cmp(b)
            }
        };
        let filter = query.r#where.as_ref().map(|f| compile_filter(f, interner));
        let mut rows: Vec<(Vec<Vec<u8>>, RecordId, Bytes)> = Vec::new();
        let mut scanned = 0;
        let mut corrupt = Vec::new();
        let stream = self
            .list_stream_tx_after_predicate(tx, shamir_tunables::store_defaults::FULL_SCAN_BATCH);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            'rows: for (id, cow) in batch? {
                scanned += 1;
                let record = match cow.into_inner() {
                    Ok(record) => record,
                    Err(_) => {
                        corrupt.push(CorruptRecordRef {
                            table: self.name().to_string(),
                            id,
                        });
                        continue;
                    }
                };
                if filter.as_ref().is_some_and(|f| !f.matches(&record, ctx)) {
                    continue;
                }
                let mut values = Vec::with_capacity(paths.len());
                for (path, item) in paths.iter().zip(&order.items) {
                    if item.field.len() == 1 && item.field[0] == "_id" {
                        // RecordId's total order is its raw 16-byte order, not
                        // the lexicographic order of its Base58 display string.
                        values.push(id.as_bytes().to_vec());
                        continue;
                    }
                    let Some(value) = path
                        .as_ref()
                        .and_then(|p| crate::query::filter::eval::resolve_field(&record, p))
                    else {
                        // Absent fields have no sorted-index posting either.
                        continue 'rows;
                    };
                    // Match sorted-index extract_and_encode exactly: only
                    // these stored scalar types have postings. In particular,
                    // do not convert maps/sets through the interner first.
                    let value = match value {
                        InnerValue::Null => QueryValue::Null,
                        InnerValue::Bool(value) => QueryValue::Bool(value),
                        InnerValue::Int(value) => QueryValue::Int(value),
                        InnerValue::F64(value) => QueryValue::F64(value),
                        InnerValue::Str(value) => QueryValue::Str(value),
                        InnerValue::Bin(value) => QueryValue::Bin(value),
                        _ => continue 'rows,
                    };
                    let Some(encoded) = encode_query_value_for_sort(&value) else {
                        // Non-indexable values also have no sorted-index posting.
                        continue 'rows;
                    };
                    values.push(encoded);
                }
                let cmp = compare(&values, &seek);
                if keyset.is_none()
                    || cmp.is_gt()
                    || (cmp.is_eq()
                        && query
                            .pagination
                            .after_id()
                            .is_some_and(|after| compare_ids(&id, after).is_gt()))
                {
                    let bytes = record
                        .to_bytes()
                        .map_err(|e| DbError::Codec(format!("Failed to encode record: {e}")))?;
                    rows.push((values, id, bytes));
                }
            }
        }
        rows.sort_unstable_by(|(a, aid, _), (b, bid, _)| {
            compare(a, b).then_with(|| compare_ids(aid, bid))
        });
        let matched: Vec<_> = rows.into_iter().map(|(_, id, bytes)| (id, bytes)).collect();
        let (page, pagination) =
            exec::apply_pagination(matched, &query.pagination, query.count_total);
        let values = apply_select_value_bytes(
            &page,
            &query.select,
            interner,
            ctx,
            self.name(),
            &mut corrupt,
        )?;
        let ids: Vec<_> = page.iter().map(|(id, _)| *id).collect();
        let records: Vec<_> = ids
            .iter()
            .copied()
            .zip(values)
            .map(|(id, fields)| {
                QueryRecord::Inserted(shamir_query_types::write::InsertedRecord {
                    id: Some(id),
                    fields,
                })
            })
            .collect();
        let returned = records.len() as u64;
        Ok(QueryResult::with_stats(
            records,
            QueryStats {
                index_used: None,
                records_scanned: scanned,
                records_returned: returned,
                execution_time_us: start.elapsed().as_micros() as u64,
            },
        )
        .with_pagination(pagination)
        .with_versions(collect_versions(
            query.with_version,
            self.mvcc_store_ref(),
            &ids,
        ))
        .with_corrupt_records(corrupt))
    }
}
