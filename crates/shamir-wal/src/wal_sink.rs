use shamir_storage::error::DbResult;

use crate::wal_entry_v2::WalEntryV2;
use crate::wal_segment::WalSegment;

/// WAL storage sink — file-backed or no-op (in-memory repos).
/// Enum, not trait: no dyn dispatch on the hot path.
#[allow(dead_code)]
pub enum WalSink {
    /// Real append-only file. write() = level 2, sync_all = level 3.
    File(WalSegment),
    /// In-memory repos: durability is meaningless (process crash loses
    /// RAM anyway). append = instant Ok, sync = no-op, replay = empty.
    Noop,
}

#[allow(dead_code)]
impl WalSink {
    pub async fn append_batch(&self, payloads: Vec<Vec<u8>>) -> DbResult<u64> {
        match self {
            Self::File(seg) => seg.append_batch(payloads).await,
            Self::Noop => Ok(0),
        }
    }

    pub async fn sync(&self) -> DbResult<()> {
        match self {
            Self::File(seg) => seg.sync().await,
            Self::Noop => Ok(()),
        }
    }

    pub async fn replay(&self) -> DbResult<Vec<WalEntryV2>> {
        match self {
            Self::File(seg) => seg.replay().await,
            Self::Noop => Ok(Vec::new()),
        }
    }
}
