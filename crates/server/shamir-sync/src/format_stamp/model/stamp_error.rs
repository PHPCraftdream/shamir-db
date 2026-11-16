//! `StampError` and the corruption taxonomy (design 2.7, 3.4).

use std::path::{Path, PathBuf};
use thiserror::Error;

use super::epochs::BASE_FORMAT_EPOCH;

/// How a stamp failed to decode / why the stamp location is unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorruptKind {
    ShortFile,
    BadMagic,
    Truncated,
    BadChecksum,
    BadBody,
    EpochMismatch,
    TornTmp,
}

impl std::fmt::Display for CorruptKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::ShortFile => "ShortFile",
            Self::BadMagic => "BadMagic",
            Self::Truncated => "Truncated",
            Self::BadChecksum => "BadChecksum",
            Self::BadBody => "BadBody",
            Self::EpochMismatch => "EpochMismatch",
            Self::TornTmp => "TornTmp",
        };
        f.write_str(name)
    }
}

impl CorruptKind {
    /// Decision-table message of row 6 (`TornTmp`) or row 18 (everything else).
    pub fn message(&self, dir: &Path) -> String {
        match self {
            Self::TornTmp => format!(
                "a format stamp write in {} was interrupted (found format.stamp.tmp, \
                 no format.stamp); refusing to start. No data was modified. Restore a backup, \
                 or, if you are sure the data is intact, remove format.stamp.tmp \
                 (the directory is then treated as unstamped; see the operations guide).",
                dir.display()
            ),
            other => format!(
                "format stamp {}/format.stamp is corrupt ({}); the format epoch of \
                 the data cannot be determined; refusing to start. No data was modified. \
                 Restore a backup, or see the operations guide section on the format stamp.",
                dir.display(),
                other
            ),
        }
    }
}

/// Startup-time format error of the data directory (design 2.7).
#[derive(Debug, Error)]
pub enum StampError {
    #[error(
        "data directory {dir} has format epoch {found}, but this binary (shamir-server {}) \
         supports epoch {supported}; refusing to start. No data was modified. Use a binary \
         that supports epoch {found}, or restore a backup made by a binary that writes \
         epoch {supported}.",
        env!("CARGO_PKG_VERSION")
    )]
    NewerEpoch {
        dir: PathBuf,
        found: u64,
        supported: u64,
    },
    #[error(
        "data directory {dir} has format epoch {found}, but this binary requires epoch \
         {current}; refusing to start. No data was modified. Take a backup, then run the \
         format-migration command (R1b)."
    )]
    OlderEpoch {
        dir: PathBuf,
        found: u64,
        current: u64,
    },
    #[error("{}", kind.message(dir))]
    Corrupt { dir: PathBuf, kind: CorruptKind },
    #[error(
        "format stamp {dir}/format.stamp uses layout {layout}, which this binary ({}) cannot \
         read completely; refusing to start. No data was modified. Use the binary version \
         that wrote it or a newer one.",
        env!("CARGO_PKG_VERSION")
    )]
    NewerLayout {
        dir: PathBuf,
        layout: u16,
        epoch: u64,
    },
    #[error(
        "data directory {dir} has no format stamp, so it is treated as epoch {}, but this \
         binary requires epoch {current}; refusing to start. No data was modified. Take a \
         backup, then run the format-migration command (R1b).",
        BASE_FORMAT_EPOCH
    )]
    AbsentOlderBase { dir: PathBuf, current: u64 },
    #[error("format stamp operation on {dir} failed")]
    Io {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
