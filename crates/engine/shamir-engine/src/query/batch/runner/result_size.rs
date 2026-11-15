//! Deterministic materialization accounting, independent of the wire codec.

use crate::query::read::{QueryRecord, QueryResult};
use shamir_engine_query::query::read::value_bytes::ValueBytes;
use shamir_query_types::read::ddl::DdlOpState;
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;

/// One primary export for the estimator and incremental container formula.
pub(in crate::query::batch) struct ResultSize;

impl ResultSize {
    // Result shell 32 + records list shell/entries + 32 per record + record
    // payload. The value-walk constants live in shamir-engine-query's
    // `ValueBytes`; `value` and `sequence` delegate to it. Optional metadata
    // pays a conservative fixed shell plus dynamic strings.
    // This is bytes materialized by execution, not serialized response length,
    // allocator capacity or live RSS: filtering/dropping never gives bytes back.
    pub(in crate::query::batch) const SHELL: u64 = ValueBytes::SHELL;
    pub(in crate::query::batch) const ENTRY: u64 = ValueBytes::ENTRY;

    pub(in crate::query::batch) fn value(value: &QueryValue) -> u64 {
        ValueBytes::value(value)
    }

    pub(in crate::query::batch) fn sequence(len: usize) -> u64 {
        ValueBytes::sequence(len)
    }

    pub(in crate::query::batch) fn result(result: &QueryResult) -> u64 {
        // Destructure exhaustively so additions to QueryResult require review.
        let QueryResult {
            receipt,
            records,
            stats,
            pagination,
            value,
            explain,
            skipped: _,
            versions,
            corrupt_records,
            op_id,
            ddl_status,
        } = result;
        let mut size = 32u64.saturating_add(Self::sequence(records.len()));
        if let Some(receipt) = receipt {
            size = size.saturating_add(crate::query::batch::receipts::receipt_size(receipt));
        }
        for record in records {
            size = size.saturating_add(32);
            size = size.saturating_add(match record {
                QueryRecord::Direct(v) => Self::value(v),
                QueryRecord::IdBytes(v) => 24u64.saturating_add(v.len() as u64),
                QueryRecord::Inserted(v) => {
                    let mut size = Self::value(&v.fields);
                    if let Some(id) = &v.id {
                        size = size.saturating_add(43 + id.to_string().len() as u64);
                        if !matches!(&v.fields, QueryValue::Map(_)) {
                            size = size.saturating_add(Self::SHELL + Self::ENTRY + 6);
                        }
                    }
                    size
                }
            });
        }
        if let Some(value) = value {
            size = size.saturating_add(Self::value(value));
        }
        if let Some(stats) = stats {
            size = size.saturating_add(256);
            if let Some(index) = &stats.index_used {
                size = size.saturating_add(index.len() as u64);
            }
        }
        // PaginationInfo has only optional u64s and bools, no cursor payload.
        if pagination.is_some() {
            size = size.saturating_add(320);
        }
        if let Some(explain) = explain {
            size = size.saturating_add(256);
            if let Some(index) = &explain.index_used {
                size = size.saturating_add(index.len() as u64);
            }
        }
        if let Some(versions) = versions {
            size = size
                .saturating_add(Self::sequence(versions.len()))
                .saturating_add(16u64.saturating_mul(versions.len() as u64));
        }
        size = size.saturating_add(Self::sequence(corrupt_records.len()));
        for record in corrupt_records {
            // The table is the only unbounded field; RecordId is 16 bytes
            // (at most 22 base58 bytes), covered by the fixed 128-byte shell.
            size = size
                .saturating_add(128)
                .saturating_add(record.table.len() as u64);
        }
        if op_id.is_some() {
            size = size.saturating_add(40);
        }
        if let Some(status) = ddl_status {
            size = size.saturating_add(128);
            match status {
                DdlOpState::Failed { detail } => {
                    size = size.saturating_add(detail.len() as u64);
                }
                DdlOpState::InProgress
                | DdlOpState::Succeeded { .. }
                | DdlOpState::SucceededViaCrashRecovery { .. }
                | DdlOpState::Unknown => {}
            }
        }
        size
    }

    /// Newly assembled alias map and serialized result envelopes only. Inner
    /// payloads (including nested Batch/ForEach values) were charged at dispatch;
    /// do not traverse them again. Empty maps and empty records lists still pay.
    pub(in crate::query::batch) fn assembled(results: &TMap<String, QueryResult>) -> u64 {
        let mut size = Self::SHELL;
        for (alias, result) in results {
            size = size
                .saturating_add(Self::ENTRY + alias.len() as u64)
                .saturating_add(Self::SHELL)
                .saturating_add(Self::ENTRY + 7) // records key
                .saturating_add(Self::sequence(result.records.len()));
            for (key, present) in [
                ("receipt", result.receipt.is_some()),
                ("stats", result.stats.is_some()),
                ("pagination", result.pagination.is_some()),
                ("value", result.value.is_some()),
                ("explain", result.explain.is_some()),
                ("skipped", result.skipped),
                ("versions", result.versions.is_some()),
                ("corrupt_records", !result.corrupt_records.is_empty()),
                ("op_id", result.op_id.is_some()),
                ("ddl_status", result.ddl_status.is_some()),
            ] {
                if present {
                    size = size.saturating_add(Self::ENTRY + key.len() as u64);
                }
            }
        }
        size
    }
}
