//! Test utilities for streaming operations.
//!
//! WARNING: These functions collect ALL data into memory and should ONLY be used in tests.
//! For production code, use streaming APIs directly.

use crate::db::engine::table::Table;
use crate::db::DbResult;
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;
use futures::StreamExt;

/// Collect all records from list_stream into a Vec.
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
        result.extend(batch);
    }
    Ok(result)
}
