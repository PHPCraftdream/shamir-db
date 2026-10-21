use crate::query::batch::{BatchError, ExecScope, QueryEntry};
use crate::query::read::{QueryRecord, QueryResult, QueryStats};
use crate::query::write::WriteResult;
use serde_bytes::ByteBuf;
use shamir_query_types::batch::{ResultEncoding, TransactionInfo};
use shamir_types::codecs::interned::query_value_to_storage_bytes_into;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::TMap;

/// Preserve row encoding while attaching receipts without a second row map.
pub(in crate::query::batch) fn write_result_to_query_result_with_encoding(
    wr: WriteResult,
    encoding: ResultEncoding,
    interner: &Interner,
) -> QueryResult {
    let records = if matches!(encoding, ResultEncoding::Id) && !wr.records.is_empty() {
        let mut scratch = Vec::new();
        wr.records
            .into_iter()
            .map(|rec| {
                let intern_fn = |key: &str| {
                    interner.get_ind(key).ok_or_else(|| {
                        shamir_types::codecs::CodecError::Decode(format!(
                            "write_result_to_query_result_with_encoding: unknown field '{key}'"
                        ))
                    })
                };
                match query_value_to_storage_bytes_into(&rec.fields, &intern_fn, &mut scratch) {
                    Ok(bytes) => QueryRecord::IdBytes(ByteBuf::from(bytes.as_ref())),
                    Err(_) => QueryRecord::from(rec),
                }
            })
            .collect()
    } else {
        wr.records.into_iter().map(QueryRecord::from).collect()
    };
    QueryResult::with_stats(
        records,
        QueryStats {
            index_used: None,
            records_scanned: wr.affected,
            records_returned: wr.affected,
            execution_time_us: wr.execution_time_us,
        },
    )
}

/// Reserve nested receipt shells before the commit decision, never turn a
/// known publication into a resource-limit error while attaching metadata.
pub(in crate::query::batch) fn reserve(
    results: &TMap<String, QueryResult>,
    queries: &TMap<String, QueryEntry>,
    depth: usize,
    scope: &ExecScope,
) -> Result<(), BatchError> {
    if depth > 0 && scope.enforce_result_size() {
        for (alias, result) in results {
            if !result.skipped && queries.get(alias).is_some_and(|entry| entry.op.is_write()) {
                scope.charge_bytes(1024)?;
            }
        }
    }
    Ok(())
}

pub(in crate::query::batch) fn receipt_size(receipt: &TransactionInfo) -> u64 {
    512u64
        .saturating_add(receipt.status.len() as u64)
        .saturating_add(receipt.error.as_ref().map_or(0, |s| s.len() as u64))
        .saturating_add(receipt.reason.as_ref().map_or(0, |s| s.len() as u64))
}
