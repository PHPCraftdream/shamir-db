//! Persistent registry of wire-created tables.
//!
//! `shamir-db`'s system store records databases and repositories but NOT
//! per-table configuration — `RepoInstance::add_table` is an in-memory
//! operation. To make tables created over the wire (`BatchOp::CreateTable`)
//! survive a server restart, this module maintains a small MessagePack file
//! at `<data_dir>/wire_tables.mpack` listing every table per `(db, repo)`.
//! The boot path replays the file by calling `DbInstance::create_table` on
//! each entry before the server starts accepting connections.
//!
//! The registry is updated by `ShamirDbHandler` AFTER a batch executes
//! successfully — so a planner-rejected or query-failing batch never
//! pollutes the file.
//!
//! ## File format
//!
//! MessagePack-encoded [`RegistrySnapshot`] produced by
//! `rmp_serde::to_vec_named`. The on-disk structure is a map from
//! `"db.repo"` string keys to arrays of table name strings.
//!
//! **Legacy:** a stale `wire_tables.legacy` (text-encoded registry from a
//! previous server version) is silently ignored; tables it referenced must be
//! re-created over the wire (or via a migration) after the upgrade.
//!
//! Atomic writes: `tempfile::NamedTempFile::persist` swaps the file in
//! place so a crash mid-write leaves either the old version or the new
//! version, never a half-written one.
//!
//! Concurrency: the registry is mutex-guarded; callers acquire the lock
//! for read-modify-write cycles.

use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};

/// File name relative to `data_dir`.
pub const FILENAME: &str = "wire_tables.mpack";

/// In-memory + on-disk view of `(db, repo) -> [table_names]`.
///
/// Keys are `format!("{db}.{repo}")` for human-readable on-disk keys.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    /// `"db.repo" -> sorted list of table names`. Sorted on every write so
    /// diffs are stable.
    #[serde(flatten)]
    pub tables_by_repo: IndexMap<String, Vec<String>>,
}

impl RegistrySnapshot {
    fn key(db: &str, repo: &str) -> String {
        format!("{db}.{repo}")
    }

    /// Add `table` under `(db, repo)`. Returns `true` if it was newly added.
    pub fn add(&mut self, db: &str, repo: &str, table: &str) -> bool {
        let entry = self.tables_by_repo.entry(Self::key(db, repo)).or_default();
        if entry.iter().any(|t| t == table) {
            false
        } else {
            entry.push(table.to_string());
            entry.sort();
            true
        }
    }

    /// Remove `table` from `(db, repo)`. Returns `true` if it was present.
    pub fn remove(&mut self, db: &str, repo: &str, table: &str) -> bool {
        if let Some(entry) = self.tables_by_repo.get_mut(&Self::key(db, repo)) {
            if let Some(pos) = entry.iter().position(|t| t == table) {
                entry.remove(pos);
                return true;
            }
        }
        false
    }

    /// Iterator over `(db, repo, table)` triples for boot replay.
    pub fn iter_entries(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.tables_by_repo
            .iter()
            .filter_map(|(key, tables)| {
                let (db, repo) = key.split_once('.')?;

                Some((db, repo, tables))
            })
            .flat_map(|(db, repo, tables)| tables.iter().map(move |t| (db, repo, t.as_str())))
    }
}

/// Mutex-guarded handle pinned to a file path. Cloneable across tasks.
#[derive(Debug, Clone)]
pub struct TablesRegistry {
    path: PathBuf,
    state: Arc<Mutex<RegistrySnapshot>>,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("registry io: {0}")]
    Io(#[from] io::Error),
    #[error("registry decode: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error("registry encode: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
}

impl TablesRegistry {
    /// Open or create the registry at `<data_dir>/wire_tables.mpack`. A
    /// missing file is treated as an empty registry.
    pub fn open(data_dir: &Path) -> Result<Self, RegistryError> {
        let path = data_dir.join(FILENAME);
        let snapshot = if path.exists() {
            let bytes = fs::read(&path)?;
            if bytes.is_empty() {
                RegistrySnapshot::default()
            } else {
                rmp_serde::from_slice(&bytes)?
            }
        } else {
            RegistrySnapshot::default()
        };
        Ok(Self {
            path,
            state: Arc::new(Mutex::new(snapshot)),
        })
    }

    /// Read-only view (for boot replay).
    pub fn snapshot(&self) -> RegistrySnapshot {
        self.state.lock().clone()
    }

    /// Add `table` under `(db, repo)` and persist.
    pub fn add(&self, db: &str, repo: &str, table: &str) -> Result<(), RegistryError> {
        let mut guard = self.state.lock();
        if !guard.add(db, repo, table) {
            return Ok(()); // already present
        }
        Self::write_atomic(&self.path, &guard)
    }

    /// Remove `table` from `(db, repo)` and persist.
    pub fn remove(&self, db: &str, repo: &str, table: &str) -> Result<(), RegistryError> {
        let mut guard = self.state.lock();
        if !guard.remove(db, repo, table) {
            return Ok(());
        }
        Self::write_atomic(&self.path, &guard)
    }

    /// Atomic write: serialize to a temp file in the same directory, then
    /// rename over the live file. On crash, either the previous or the new
    /// content is left, never a partial write.
    fn write_atomic(path: &Path, snapshot: &RegistrySnapshot) -> Result<(), RegistryError> {
        let bytes = rmp_serde::to_vec_named(snapshot)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        // tempfile::NamedTempFile lives in this same directory so the
        // rename in `persist` is atomic on the same filesystem.
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(&bytes)?;
        tmp.flush()?;
        // `persist` performs an atomic rename on Unix; on Windows it falls
        // back to MoveFileExW(MOVEFILE_REPLACE_EXISTING) which is also
        // atomic with respect to readers.
        tmp.persist(path).map_err(|e| RegistryError::Io(e.error))?;
        Ok(())
    }
}
