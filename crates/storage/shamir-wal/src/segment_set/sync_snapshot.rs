//! Exact replay snapshot for the detached durability worker.

use std::fs::File;
use std::io::Read;

use super::set::*;
use crate::segment::frame_reader::{FrameRead, FrameReader};

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
            #[cfg(test)]
            let injected = self
                .sync_test
                .sealed_error
                .swap(0, std::sync::atomic::Ordering::AcqRel);
            let records = spawn_blocking(move || -> DbResult<Vec<WalEntryV2>> {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(!synced)
                    .open(&path)
                    .map_err(|e| wal_io_error("sealed snapshot open".into(), e))?;
                #[cfg(test)]
                if injected == 1 {
                    return Err(wal_io_error(
                        "sealed snapshot read".into(),
                        super::sync_test_seam::injected_error(),
                    ));
                }
                let length = file
                    .metadata()
                    .map_err(|e| wal_io_error("sealed snapshot metadata".into(), e))?
                    .len();
                let records = decode_snapshot(&file, length)?;
                if !synced {
                    #[cfg(test)]
                    let sync_result = if injected == 2 {
                        Err(super::sync_test_seam::injected_error())
                    } else {
                        file.sync_all()
                    };
                    #[cfg(not(test))]
                    let sync_result = file.sync_all();
                    sync_result.map_err(|e| wal_io_error("sealed snapshot sync".into(), e))?;
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
                .map_err(|e| wal_io_error(format!("sync snapshot open {path:?}"), e))?;
            decode_snapshot(file, bytes)
        })
        .await
        .map_err(|e| DbError::Internal(format!("sync snapshot join: {e}")))??;
        entries.extend(captured);
        Ok((entries, active))
    }
}

fn decode_snapshot(reader: impl Read, length: u64) -> DbResult<Vec<WalEntryV2>> {
    let mut reader = FrameReader::new(reader, length, "WAL snapshot read".into());
    let mut out = Vec::new();
    loop {
        match reader.next()? {
            FrameRead::End => return Ok(out),
            FrameRead::Incomplete { .. } => {
                return Err(DbError::Storage("torn WAL snapshot frame".into()));
            }
            FrameRead::Frame {
                payload, crc_ok, ..
            } => {
                if !crc_ok {
                    return Err(DbError::Storage("WAL snapshot checksum mismatch".into()));
                }
                out.push(WalEntryV2::decode(&payload)?);
            }
        }
    }
}
