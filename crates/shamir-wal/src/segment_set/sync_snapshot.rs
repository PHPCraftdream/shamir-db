//! Exact replay snapshot for the detached durability worker.

use super::set::*;
use std::fs::File;
use std::io::Read;

impl SegmentSet {
    /// The coordinator holds sync_gate through snapshot/fsync; history-backed
    /// group truncation holds it through replay/publication/unlink. Direct sink
    /// unlink remains fail-closed on a missing captured path.
    /// Ordinary sealed files were synced by that leader; uncertain seals (open
    /// or poison) are explicitly synced here, never treated as durable by name.
    pub(crate) async fn sync_snapshot(&self) -> DbResult<(Vec<WalEntryV2>, Arc<WalSegment>)> {
        let (sealed, active, path) = {
            let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            (
                g.sealed.clone(),
                Arc::clone(&g.active),
                self.dir.join(seg_file_name(g.active_seq)),
            )
        };
        #[cfg(test)]
        self.sync_test.snapshot().await;
        // bytes_written is published only after successful write_all. Reading
        // beyond it could certify a complete frame from a batch later rolled
        // back. The sole append leader never rolls back below this boundary.
        let bytes = active.approx_len_bytes();
        let mut entries = Vec::new();
        for meta in sealed {
            // Never create/repair a live sealed path. Concurrent truncation
            // fails this round closed; durable history publication permits retry.
            let path = meta.path;
            let synced = meta.synced;
            let records = spawn_blocking(move || -> DbResult<Vec<WalEntryV2>> {
                let mut file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(!synced)
                    .open(&path)
                    .map_err(|e| DbError::Storage(format!("sealed snapshot open: {e}")))?;
                let mut buffer = Vec::new();
                file.read_to_end(&mut buffer)
                    .map_err(|e| DbError::Storage(format!("sealed snapshot read: {e}")))?;
                let records = decode_snapshot(&buffer)?;
                if !synced {
                    file.sync_all()
                        .map_err(|e| DbError::Storage(format!("sealed snapshot sync: {e}")))?;
                }
                Ok(records)
            })
            .await
            .map_err(|e| DbError::Internal(format!("sealed snapshot join: {e}")))??;
            if !synced {
                let mut g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
                if let Some(sealed) = g.sealed.iter_mut().find(|m| m.seq == meta.seq) {
                    sealed.synced = true;
                }
            }
            entries.extend(records);
        }
        let captured = spawn_blocking(move || -> DbResult<Vec<WalEntryV2>> {
            let file = File::open(&path)
                .map_err(|e| DbError::Storage(format!("sync snapshot open {path:?}: {e}")))?;
            let mut buffer = Vec::new();
            file.take(bytes)
                .read_to_end(&mut buffer)
                .map_err(|e| DbError::Storage(format!("sync snapshot read: {e}")))?;
            if u64::try_from(buffer.len()).ok() != Some(bytes) {
                return Err(DbError::Storage("short WAL sync snapshot".into()));
            }
            decode_snapshot(&buffer)
        })
        .await
        .map_err(|e| DbError::Internal(format!("sync snapshot join: {e}")))??;
        entries.extend(captured);
        Ok((entries, active))
    }
}

fn decode_snapshot(buffer: &[u8]) -> DbResult<Vec<WalEntryV2>> {
    let mut out = Vec::new();
    let mut position = 0usize;
    while position < buffer.len() {
        let header = buffer
            .get(position..position.saturating_add(4))
            .ok_or_else(|| DbError::Storage("torn WAL snapshot header".into()))?;
        let length = u32::from_le_bytes(header.try_into().expect("four-byte header")) as usize;
        let start = position + 4;
        let end = start
            .checked_add(length)
            .ok_or_else(|| DbError::Storage("WAL snapshot length overflow".into()))?;
        let payload = buffer
            .get(start..end)
            .ok_or_else(|| DbError::Storage("torn WAL snapshot payload".into()))?;
        let crc = buffer
            .get(end..end.saturating_add(4))
            .ok_or_else(|| DbError::Storage("torn WAL snapshot checksum".into()))?;
        if crc32fast::hash(payload)
            != u32::from_le_bytes(crc.try_into().expect("four-byte checksum"))
        {
            return Err(DbError::Storage("WAL snapshot checksum mismatch".into()));
        }
        out.push(WalEntryV2::decode(payload)?);
        position = end + 4;
    }
    Ok(out)
}
