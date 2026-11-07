use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::replication_seed::codec::{hash_reader, ARTIFACT_LIMIT, METADATA_LIMIT};
use crate::replication_seed::encode_bounded::encode_bounded;
use crate::replication_seed::error::invalid;
use crate::replication_seed::{FileRecord, SeedError};

pub(crate) const FILE_LIMIT: usize = 65_536;

/// Exclusive, bounded file creation. Failed attempts are deliberately preserved.
pub(crate) struct ArtifactWriter {
    pub root: PathBuf,
    pub files: Vec<FileRecord>,
    bytes: u64,
    metadata_bytes: u64,
    file_limit: usize,
    metadata_limit: u64,
}

impl ArtifactWriter {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: Vec::new(),
            bytes: 0,
            // Reserve the largest MessagePack array header.
            metadata_bytes: 5,
            file_limit: FILE_LIMIT,
            metadata_limit: METADATA_LIMIT,
        }
    }

    #[cfg(test)]
    pub fn with_limits(root: PathBuf, file_limit: usize, metadata_limit: u64) -> Self {
        Self {
            file_limit,
            metadata_limit,
            ..Self::new(root)
        }
    }

    pub fn file(
        &mut self,
        path: String,
        bytes: &[u8],
        table: Option<u32>,
        rows: u64,
    ) -> Result<(), SeedError> {
        if self.files.len() >= self.file_limit {
            return Err(invalid("artifact file count budget exceeded"));
        }
        let size = u64::try_from(bytes.len()).map_err(|e| invalid(e.to_string()))?;
        let total = self
            .bytes
            .checked_add(size)
            .ok_or_else(|| invalid("artifact byte overflow"))?;
        if total > ARTIFACT_LIMIT {
            return Err(invalid("artifact byte budget exceeded"));
        }
        // Reserve the exact serialized record size before filesystem mutation
        // or growing the retained inventory. SHA-256 always has 64 hex bytes.
        let mut record = FileRecord {
            path,
            bytes: size,
            sha256: "0".repeat(64),
            table,
            rows,
        };
        let remaining = self
            .metadata_limit
            .checked_sub(self.metadata_bytes)
            .ok_or_else(|| invalid("file metadata budget exceeded"))?;
        let encoded = encode_bounded(
            &record,
            usize::try_from(remaining).map_err(|e| invalid(e.to_string()))?,
        )
        .map_err(|_| invalid("file metadata budget exceeded"))?;
        let entry_bytes = u64::try_from(encoded.len()).map_err(|e| invalid(e.to_string()))?;
        let metadata_total = self
            .metadata_bytes
            .checked_add(entry_bytes)
            .ok_or_else(|| invalid("file metadata overflow"))?;
        if metadata_total > self.metadata_limit {
            return Err(invalid("file metadata budget exceeded"));
        }
        write_new(&self.root.join(&record.path), bytes)?;
        let (observed_size, sha256) = hash_reader(&mut File::open(self.root.join(&record.path))?)?;
        if observed_size != size {
            return Err(invalid("artifact file changed during write"));
        }
        record.sha256 = sha256;
        self.bytes = total;
        self.metadata_bytes = metadata_total;
        self.files.push(record);
        Ok(())
    }
}

/// Actual exporter completion boundary. The callback can fail deterministically
/// after all data files exist, but before either completion file is published.
pub(crate) fn publish_manifest(
    root: &Path,
    bytes: &[u8],
    digest: &str,
    before_manifest: impl FnOnce() -> Result<(), SeedError>,
) -> Result<(), SeedError> {
    before_manifest()?;
    write_new(&root.join("Complete"), digest.as_bytes())?;
    write_new(&root.join("manifest.mpack"), bytes)?;
    Ok(())
}

pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), SeedError> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<(), SeedError> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(invalid(
            "durable seed publication is supported only on Unix",
        ))
    }
}
