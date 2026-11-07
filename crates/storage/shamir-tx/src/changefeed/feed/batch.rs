//! Bounded event-only journal batches; control messages are FIFO boundaries.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::Notify;

use super::core::{process_journal_msg, serialize_event, ChangelogStore, GapMarker, JournalMsg};
use super::error::ChangelogError;
use super::events::{version_key, ChangelogEvent};
use crate::changefeed::repair_pending::RepairPending;

const MAX_EVENTS: usize = 256;
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(super) struct EventBatch {
    items: Vec<(Bytes, Bytes)>,
    events: Vec<Arc<ChangelogEvent>>,
    bytes: usize,
}

impl EventBatch {
    pub(super) async fn flush(
        &mut self,
        store: &Arc<dyn ChangelogStore>,
        watermark: &AtomicU64,
        gap: &GapMarker,
    ) {
        if self.items.is_empty() {
            return;
        }
        let items = std::mem::take(&mut self.items);
        self.bytes = 0;
        let mut results = store.put_many(items).await.into_iter();
        for event in self.events.drain(..) {
            match results.next() {
                Some(Ok(())) => {
                    watermark.fetch_max(event.commit_version, Ordering::Release);
                }
                Some(Err(error)) => {
                    gap.mark_gap(event.commit_version);
                    log::warn!("changefeed writer: journal put failed for repo {} commit_version {}: {error}", event.repo, event.commit_version);
                }
                None => {
                    gap.mark_gap(event.commit_version);
                    log::warn!(
                        "changefeed writer: batch omitted result for commit_version {}",
                        event.commit_version
                    );
                }
            }
        }
    }

    pub(super) async fn push(
        &mut self,
        store: &Arc<dyn ChangelogStore>,
        msg: JournalMsg,
        state: (
            &Arc<AtomicU64>,
            &GapMarker,
            &AtomicU64,
            &Notify,
            &RepairPending,
        ),
    ) {
        let (watermark, gap, confirmed, notify, pending) = state;
        match msg {
            JournalMsg::Event(event) => {
                let encoded = serialize_event(&event);
                self.push_encoded(store, event, encoded, watermark, gap)
                    .await;
            }
            control => {
                self.flush(store, watermark, gap).await;
                process_journal_msg(store, control, watermark, gap, confirmed, notify, pending)
                    .await;
            }
        }
    }

    // This common path also accepts a serialization error: it must not let
    // the failed event's gap overtake an earlier pending event.
    async fn push_encoded(
        &mut self,
        store: &Arc<dyn ChangelogStore>,
        event: Arc<ChangelogEvent>,
        encoded: Result<Bytes, ChangelogError>,
        watermark: &AtomicU64,
        gap: &GapMarker,
    ) {
        let value = match encoded {
            Ok(value) => value,
            Err(error) => {
                self.flush(store, watermark, gap).await;
                gap.mark_gap(event.commit_version);
                log::warn!("changefeed writer: {error}; dropping journal entry");
                return;
            }
        };
        let key = version_key(event.commit_version);
        let size = key.len().saturating_add(value.len());
        if !self.items.is_empty()
            && (self.items.len() >= MAX_EVENTS || self.bytes.saturating_add(size) > MAX_BYTES)
        {
            self.flush(store, watermark, gap).await;
        }
        // An oversized event is persisted alone, rather than rejected or
        // combined with another event; the budget bounds multi-event batches.
        self.bytes = self.bytes.saturating_add(size);
        self.items.push((key, value));
        self.events.push(event);
        if self.items.len() >= MAX_EVENTS || self.bytes >= MAX_BYTES {
            self.flush(store, watermark, gap).await;
        }
    }
}

#[cfg(test)]
mod tests;
