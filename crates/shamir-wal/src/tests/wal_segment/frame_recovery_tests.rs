//! Bounded recovery and oversized frame regressions.

use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use tempfile::TempDir;

use crate::segment::frame_reader::{FrameRead, FrameReader};
use crate::wal_entry_v2::WalEntryV2;
use crate::wal_segment::WalSegment;

struct ShortReader<R>(R);

impl<R: Read> Read for ShortReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let limit = buf.len().min(1);
        self.0.read(&mut buf[..limit])
    }
}

#[test]
fn rv_wal_cc3131a9_frame_reader_handles_short_reads() {
    let payload = b"abc";
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    let len = bytes.len() as u64;
    let mut reader = FrameReader::new(ShortReader(bytes.as_slice()), len, "short read".into());
    match reader.next().unwrap() {
        FrameRead::Frame {
            payload, crc_ok, ..
        } => {
            assert!(crc_ok);
            assert_eq!(payload, b"abc");
        }
        _ => panic!("expected complete frame"),
    }
    assert!(matches!(reader.next().unwrap(), FrameRead::End));
}

#[tokio::test]
async fn rv_wal_cc3131a9_open_rejects_oversized_active_frame() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("00000000.wal");
    let len = 64 * 1024 * 1024 + 1u32;
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(&len.to_le_bytes()).unwrap();
    file.set_len(u64::from(len) + 8).unwrap();
    drop(file);
    let set = crate::segment_set::SegmentSet::open(dir.path().to_path_buf(), u64::MAX).await;
    assert!(set.is_err());
    assert_eq!(std::fs::metadata(path).unwrap().len(), u64::from(len) + 8);
}

#[tokio::test]
async fn rv_wal_cc3131a9_oversized_sparse_frame_never_truncated() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("segment.wal");
    let length = 64 * 1024 * 1024 + 1u32;
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(&length.to_le_bytes()).unwrap();
    file.set_len(u64::from(length) + 8).unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    file.write_all(&[0xa5]).unwrap();
    drop(file);
    let seg = WalSegment::open(path.clone()).await.unwrap();
    let original_len = std::fs::metadata(&path).unwrap().len();
    assert!(seg.replay().await.is_err());
    assert!(seg.replay_sealed().await.is_err());
    assert!(seg.repair_torn_tail().await.is_err());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), original_len);
    let mut file = std::fs::File::open(path).unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    let mut last = [0];
    file.read_exact(&mut last).unwrap();
    assert_eq!(last, [0xa5]);
}

#[tokio::test]
async fn rv_wal_cc3131a9_oversized_append_refused_before_mutation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("segment.wal");
    let seg = WalSegment::open(path.clone()).await.unwrap();
    let mut payload = WalEntryV2::new(1, 0, Vec::new()).encode().unwrap();
    payload.resize(64 * 1024 * 1024 + 1, 0);
    assert!(seg.append_batch(Arc::new(vec![payload]), 1).await.is_err());
    assert_eq!(seg.approx_len_bytes(), 0);
    assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
}

#[tokio::test]
async fn rv_wal_cc3131a9_oversized_header_is_not_repaired_as_torn_tail() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("segment.wal");
    std::fs::write(&path, u32::MAX.to_le_bytes()).unwrap();
    let seg = WalSegment::open(path.clone()).await.unwrap();
    assert!(seg.replay().await.is_err());
    assert!(seg.repair_torn_tail().await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), u32::MAX.to_le_bytes());
}
