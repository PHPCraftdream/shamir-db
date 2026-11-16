//! Read-only stamp observation for `backup` and the snapshot-stamp gate
//! for `restore` (design 3.5, 5.2). Neither function ever writes.

use std::fmt;
use std::path::Path;

use crate::format_stamp::io::{inspect, StampState};
use crate::format_stamp::model::{EpochPolicy, StampError};

/// What `backup` observed about the source stamp before copying (3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupStampObservation {
    /// A valid stamp: its format epoch.
    Epoch(u64),
    /// No stamp file (pre-stamp / base-epoch data).
    Unstamped,
    /// A stamp exists but cannot be decoded; it is copied as is.
    Unreadable,
}

impl fmt::Display for BackupStampObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Epoch(e) => write!(f, "epoch {e}"),
            Self::Unstamped => f.write_str("unstamped"),
            Self::Unreadable => f.write_str("unreadable"),
        }
    }
}

/// Observe the stamp of `dir` read-only; refuses nothing.
pub fn observe_stamp_for_backup(dir: &Path) -> BackupStampObservation {
    match inspect(dir) {
        StampState::Valid(stamp) => BackupStampObservation::Epoch(stamp.format_epoch()),
        StampState::Absent { .. } => BackupStampObservation::Unstamped,
        StampState::Corrupt { .. } | StampState::NewerLayout { .. } => {
            BackupStampObservation::Unreadable
        }
    }
}

/// `restore`'s snapshot-stamp gate (3.5): newer, corrupt or newer-layout
/// refuses; absent or older is allowed (rollback must work). Never writes:
/// the stamp inside the copy is the source's.
pub fn restore_snapshot_gate(snapshot: &Path, policy: &EpochPolicy) -> Result<(), StampError> {
    match inspect(snapshot) {
        StampState::Absent { .. } => Ok(()),
        StampState::Valid(stamp) => {
            let found = stamp.format_epoch();
            if found > policy.current {
                Err(StampError::NewerEpoch {
                    dir: snapshot.to_path_buf(),
                    found,
                    supported: policy.current,
                })
            } else {
                Ok(())
            }
        }
        StampState::Corrupt { kind } => Err(StampError::Corrupt {
            dir: snapshot.to_path_buf(),
            kind,
        }),
        StampState::NewerLayout { layout, epoch } => Err(StampError::NewerLayout {
            dir: snapshot.to_path_buf(),
            layout,
            epoch,
        }),
    }
}
