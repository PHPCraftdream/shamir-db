//! Strict (fail-closed) partition scan on `TableManager`.
//!
//! The lenient twin lives in `table_manager/accessors/table_manager_streaming.rs`;
//! that folder is at the 7-entry layout limit and the file at 990 of its 1000
//! lines, so the strict variant has its own file here.

use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_types::types::record_id::RecordId;

use crate::record_cow::RecordCow;
use crate::support::model::table::Table;
use crate::table_manager::TableManager;

impl TableManager {
    /// Strict twin of [`TableManager::list_stream`]: same batches, but a
    /// physical key that is not a 16-byte record id is yielded as
    /// `Err(DbError::Codec)` for that batch instead of being skipped. That
    /// error does NOT end the stream: the next poll continues with the next
    /// batch, and it is the consumer that stops — `read_journal_partition_records`
    /// applies `?` to the batch, because the journal treats one unreadable row
    /// as a whole-open refusal.
    ///
    /// Used by EVERY reader of the migration-journal partition (`shamir-db`'s
    /// `read_journal_partition_records`: the startup routing scan, the history
    /// read and the control lookup), not only the open scan. Every ordinary
    /// read keeps the lenient [`TableManager::list_stream`]: a corrupted key in
    /// an unrelated partition must not break those scans.
    pub fn list_stream_strict(
        &self,
        batch_size: usize,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + '_ {
        type DynStream<'a> = std::pin::Pin<
            Box<dyn futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + Send + 'a>,
        >;
        if let Err(error) = self.ensure_repo_readable() {
            let failed: DynStream<'_> = Box::pin(futures::stream::once(async move { Err(error) }));
            return failed;
        }
        if let Some(mvcc) = self.mvcc_store_ref() {
            let mvcc = Arc::clone(mvcc);
            let s: DynStream<'_> = Box::pin(async_stream::stream! {
                let mut raw = mvcc.current_stream(batch_size);
                while let Some(batch_result) = raw.next().await {
                    let batch_bytes = match batch_result {
                        Ok(batch) => batch,
                        Err(error) => {
                            yield Err(error);
                            return;
                        }
                    };
                    yield Table::decode_raw_batch_strict(batch_bytes);
                }
            });
            s
        } else {
            let s: DynStream<'_> = Box::pin(self.table.list_stream_strict(batch_size));
            s
        }
    }
}
