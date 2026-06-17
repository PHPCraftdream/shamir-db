//! Test utilities for streaming operations.
//!
//! WARNING: These functions collect ALL data into memory and should ONLY be used in tests.
//! For production code, use streaming APIs directly.

#![allow(deprecated)] // this module's own collectors are deprecated by design; internal cross-use is intentional

use crate::table::record_cow::RecordCow;
use crate::table::Table;
use futures::StreamExt;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

/// Collect all records from list_stream into a Vec, decoding `RecordCow`
/// items to `InnerValue`.
///
/// # Warning
/// FOR TESTS ONLY! This function loads ALL records into memory.
/// Can cause OOM on large datasets. Use `list_stream()` directly in production.
#[deprecated(
    since = "0.1.0",
    note = "FOR TESTS ONLY. Can consume all memory on large datasets."
)]
pub async fn collect_list_stream(table: &Table) -> DbResult<Vec<(RecordId, InnerValue)>> {
    let mut result = Vec::new();
    let stream = table.list_stream(1000);
    futures::pin_mut!(stream);
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for (id, cow) in batch {
            let inner = match cow {
                RecordCow::Borrowed(b) => {
                    InnerValue::from_bytes(b).map_err(|e| DbError::Codec(e.to_string()))?
                }
                RecordCow::Owned(v) => v,
            };
            result.push((id, inner));
        }
    }
    Ok(result)
}

/// Collect all records from a filter_stream (RecordCow) into a flat Vec of
/// `(RecordId, InnerValue)`, decoding borrowed rows.
///
/// # Warning
/// FOR TESTS ONLY! This function loads ALL filtered records into memory.
#[deprecated(
    since = "0.1.0",
    note = "FOR TESTS ONLY. Can consume all memory on large datasets."
)]
pub async fn collect_filter_stream(
    stream: impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>>,
) -> DbResult<Vec<(RecordId, InnerValue)>> {
    let mut result = Vec::new();
    futures::pin_mut!(stream);
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for (id, cow) in batch {
            let inner = match cow {
                RecordCow::Borrowed(b) => {
                    InnerValue::from_bytes(b).map_err(|e| DbError::Codec(e.to_string()))?
                }
                RecordCow::Owned(v) => v,
            };
            result.push((id, inner));
        }
    }
    Ok(result)
}
