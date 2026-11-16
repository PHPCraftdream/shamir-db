use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::format_stamp::io::atomic_write::fsync_dir;

use super::reader::parse_records_with_prefix;
use super::record::{JournalRecord, JOURNAL_FILE_NAME};
use crate::format_stamp::migration::MigrationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalMode {
    Live,
    PlanOnly,
}

/// Synchronous append-only journal handle borrowed by format-migration steps.
pub struct JournalWriter {
    dir: PathBuf,
    path: PathBuf,
    mode: JournalMode,
}

impl JournalWriter {
    /// Create or truncate the data-directory journal and persist its creation.
    pub fn create(dir: &Path) -> Result<Self, MigrationError> {
        let path = dir.join(JOURNAL_FILE_NAME);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|source| journal_error(dir, source))?;
        file.sync_all()
            .map_err(|source| journal_error(dir, source))?;
        fsync_dir(dir).map_err(|source| journal_error(dir, source))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            path,
            mode: JournalMode::Live,
        })
    }

    /// Open an existing journal, discarding a torn final suffix so crash-safe resume appends after the last complete record.
    pub fn open_existing(dir: &Path) -> Result<Self, MigrationError> {
        let path = dir.join(JOURNAL_FILE_NAME);
        let bytes = fs::read(&path).map_err(|source| journal_error(dir, source))?;
        let (_, accepted_bytes) = parse_records_with_prefix(&bytes)
            .map_err(|error| journal_error(dir, std::io::Error::other(error.message)))?;
        if accepted_bytes != bytes.len() {
            let file = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(|source| journal_error(dir, source))?;
            let length = u64::try_from(accepted_bytes).map_err(|_| {
                journal_error(dir, std::io::Error::other("journal length exceeds u64"))
            })?;
            file.set_len(length)
                .map_err(|source| journal_error(dir, source))?;
            file.sync_all()
                .map_err(|source| journal_error(dir, source))?;
        }
        OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|source| journal_error(dir, source))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            path,
            mode: JournalMode::Live,
        })
    }

    /// A no-write handle for plan execution, whose context still requires a journal.
    pub fn plan_handle(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            path: dir.join(JOURNAL_FILE_NAME),
            mode: JournalMode::PlanOnly,
        }
    }

    /// Append one UTF-8 JSON record and fsync it before returning.
    pub fn append(&self, record: &JournalRecord) -> Result<(), MigrationError> {
        if self.mode == JournalMode::PlanOnly {
            // Plan mode is contractually read-only; do not create a journal as a side effect.
            return Ok(());
        }
        let mut line = serde_json::to_vec(record)
            .map_err(|error| journal_error(&self.dir, std::io::Error::other(error)))?;
        line.push(b'\n');
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|source| journal_error(&self.dir, source))?;
        file.write_all(&line)
            .map_err(|source| journal_error(&self.dir, source))?;
        file.sync_all()
            .map_err(|source| journal_error(&self.dir, source))
    }
}

fn journal_error(dir: &Path, source: std::io::Error) -> MigrationError {
    MigrationError::Journal {
        dir: dir.to_path_buf(),
        source,
    }
}
