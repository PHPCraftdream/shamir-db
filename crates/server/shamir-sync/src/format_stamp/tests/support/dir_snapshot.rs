//! Test-side recursive directory snapshot (design 6.1): listing (rel path,
//! kind, size), per-file content hash and mtime, excluding only
//! `.shamir.lock`.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    rel: String,
    kind: EntryKind,
    size: u64,
    hash: Option<String>,
    mtime_ns: Option<u128>,
}

/// Snapshot of a fixture directory; `assert_unchanged` is the
/// "refused before any write" assertion.
pub struct DirSnapshot {
    entries: Vec<Entry>,
}

impl DirSnapshot {
    pub fn take(dir: &Path) -> Self {
        let mut entries = Vec::new();
        walk(dir, "", &mut entries);
        entries.sort_by(|a, b| a.rel.cmp(&b.rel));
        Self { entries }
    }

    /// Listing, bytes and mtimes must be identical.
    pub fn assert_unchanged(&self, dir: &Path) {
        let after = Self::take(dir);
        assert_eq!(
            self.entries,
            after.entries,
            "directory changed unexpectedly in {}",
            dir.display()
        );
    }

    /// Listing, bytes and mtimes must be identical except for named files at
    /// the directory root.
    pub fn assert_unchanged_except_top_level_files(&self, dir: &Path, excluded: &[&str]) {
        let expected: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| !is_excluded_top_level_file(entry, excluded))
            .collect();
        let after = Self::take(dir);
        let actual: Vec<_> = after
            .entries
            .iter()
            .filter(|entry| !is_excluded_top_level_file(entry, excluded))
            .collect();
        assert_eq!(
            expected,
            actual,
            "directory changed unexpectedly outside excluded top-level files in {}",
            dir.display()
        );
    }

    /// Number of listed entries (excluding `.shamir.lock`).
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn is_excluded_top_level_file(entry: &Entry, excluded: &[&str]) -> bool {
    entry.kind == EntryKind::File
        && !entry.rel.contains('/')
        && excluded.contains(&entry.rel.as_str())
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<Entry>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        // The data-dir lock exists before the check in every caller.
        if rel == ".shamir.lock" {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            walk(&entry.path(), &rel, out);
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        // Fresh metadata: on Windows `DirEntry::metadata` comes from the
        // enumeration cache and can lag behind the file.
        let meta = match fs::symlink_metadata(entry.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let hash = if kind == EntryKind::File {
            fs::read(entry.path()).ok().map(|bytes| {
                let digest = Sha256::digest(&bytes);
                hex::encode(digest)
            })
        } else {
            None
        };
        // Directory mtimes are skipped: NTFS updates them lazily, and the
        // listing already records every entry created or removed inside.
        let mtime_ns = (kind != EntryKind::Dir)
            .then(|| meta.modified().ok())
            .flatten()
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            });
        out.push(Entry {
            rel,
            kind,
            size: meta.len(),
            hash,
            mtime_ns,
        });
    }
}
