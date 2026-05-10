//! Persistent registry of wire-created tables.
//!
//! `shamir-db`'s system store records databases and repositories but NOT
//! per-table configuration — `RepoInstance::add_table` is an in-memory
//! operation. To make tables created over the wire (`BatchOp::CreateTable`)
//! survive a server restart, this module maintains a small JSON file at
//! `<data_dir>/wire_tables.json` listing every table per `(db, repo)`. The
//! boot path replays the file by calling `DbInstance::create_table` on each
//! entry before the server starts accepting connections.
//!
//! The registry is updated by `ShamirDbHandler` AFTER a batch executes
//! successfully — so a planner-rejected or query-failing batch never
//! pollutes the file.
//!
//! ## File format
//!
//! ```json
//! {
//!   "default.main": ["widgets", "orders"],
//!   "default.archive": ["events"]
//! }
//! ```
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
pub const FILENAME: &str = "wire_tables.json";

/// In-memory + on-disk view of `(db, repo) -> [table_names]`.
///
/// Keys are `format!("{db}.{repo}")` so the JSON is human-readable.
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
                let mut parts = key.splitn(2, '.');
                let db = parts.next()?;
                let repo = parts.next()?;
                Some((db, repo, tables))
            })
            .flat_map(|(db, repo, tables)| {
                tables.iter().map(move |t| (db, repo, t.as_str()))
            })
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
    Decode(#[from] serde_json::Error),
}

impl TablesRegistry {
    /// Open or create the registry at `<data_dir>/wire_tables.json`. A
    /// missing file is treated as an empty registry.
    pub fn open(data_dir: &Path) -> Result<Self, RegistryError> {
        let path = data_dir.join(FILENAME);
        let snapshot = if path.exists() {
            let bytes = fs::read(&path)?;
            if bytes.is_empty() {
                RegistrySnapshot::default()
            } else {
                serde_json::from_slice(&bytes)?
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
        Self::write_atomic(&self.path, &*guard)
    }

    /// Remove `table` from `(db, repo)` and persist.
    pub fn remove(&self, db: &str, repo: &str, table: &str) -> Result<(), RegistryError> {
        let mut guard = self.state.lock();
        if !guard.remove(db, repo, table) {
            return Ok(());
        }
        Self::write_atomic(&self.path, &*guard)
    }

    /// Atomic write: serialize to a temp file in the same directory, then
    /// rename over the live file. On crash, either the previous or the new
    /// content is left, never a partial write.
    fn write_atomic(path: &Path, snapshot: &RegistrySnapshot) -> Result<(), RegistryError> {
        let bytes = serde_json::to_vec_pretty(snapshot)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_missing_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let r = TablesRegistry::open(tmp.path()).unwrap();
        assert!(r.snapshot().tables_by_repo.is_empty());
    }

    #[test]
    fn add_and_persist_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let r = TablesRegistry::open(tmp.path()).unwrap();
        r.add("default", "main", "widgets").unwrap();
        r.add("default", "main", "orders").unwrap();
        r.add("default", "archive", "events").unwrap();

        // Reopen — file picked up.
        let r2 = TablesRegistry::open(tmp.path()).unwrap();
        let snap = r2.snapshot();
        assert_eq!(snap.tables_by_repo.get("default.main").unwrap(),
                   &vec!["orders".to_string(), "widgets".to_string()],
                   "tables sorted");
        assert_eq!(snap.tables_by_repo.get("default.archive").unwrap(),
                   &vec!["events".to_string()]);

        let entries: Vec<_> = snap.iter_entries().collect();
        assert!(entries.contains(&("default", "main", "orders")));
        assert!(entries.contains(&("default", "main", "widgets")));
        assert!(entries.contains(&("default", "archive", "events")));
    }

    #[test]
    fn add_idempotent() {
        let tmp = TempDir::new().unwrap();
        let r = TablesRegistry::open(tmp.path()).unwrap();
        r.add("d", "r", "t").unwrap();
        r.add("d", "r", "t").unwrap();
        let snap = r.snapshot();
        assert_eq!(snap.tables_by_repo.get("d.r").unwrap().len(), 1);
    }

    #[test]
    fn remove_persists() {
        let tmp = TempDir::new().unwrap();
        let r = TablesRegistry::open(tmp.path()).unwrap();
        r.add("d", "r", "t1").unwrap();
        r.add("d", "r", "t2").unwrap();
        r.remove("d", "r", "t1").unwrap();
        let r2 = TablesRegistry::open(tmp.path()).unwrap();
        assert_eq!(
            r2.snapshot().tables_by_repo.get("d.r").unwrap(),
            &vec!["t2".to_string()]
        );
    }
}
