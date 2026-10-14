use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::oneshot;

use super::{version_key, ChangelogStore, JournalMsg, RepoChangefeed};

impl RepoChangefeed {
    /// Durable install for the follower receipt protocol (S5c-X PR6b-2):
    /// write the exact pre-encoded journal `records` through the journal
    /// writer, preserving FIFO order with concurrently enqueued events.
    /// Awaiting both the enqueue (`send`, never `try_send` — a full channel
    /// backpressures instead of dropping) and the writer's acknowledgement
    /// makes this path lossless.
    pub async fn install_durable_events(
        self: &Arc<Self>,
        records: Vec<(u64, Bytes)>,
    ) -> Result<(), String> {
        let (ack, rx) = oneshot::channel();
        self.journal_tx
            .send(JournalMsg::InstallRecords { records, ack })
            .await
            .map_err(|_| "journal writer is stopped".to_string())?;
        rx.await
            .map_err(|_| "journal writer dropped the install ack".to_string())?
    }
}

pub(super) async fn process_install(
    store: &Arc<dyn ChangelogStore>,
    records: Vec<(u64, Bytes)>,
    last_persisted_version: &Arc<AtomicU64>,
    ack: oneshot::Sender<Result<(), String>>,
) {
    for (version, bytes) in records {
        if let Err(e) = store.put(version_key(version), bytes).await {
            // Deliberately no gap mark here: the receipt owner retries the
            // identical install; a failed put leaves the journal
            // byte-absent and the receipt's read-back validation never
            // certifies it, so a retry can never create a gap.
            let _ = ack.send(Err(format!("journal install put v{version}: {e}")));
            return;
        }
        last_persisted_version.fetch_max(version, Ordering::Release);
    }
    let _ = ack.send(Ok(()));
}
