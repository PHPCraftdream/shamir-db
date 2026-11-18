//! Share-tolerant recursive copy with a snapshot-defining hash pass
//! (design `online-backup.md` 4.2, steps C/D/E). WAL directories go
//! through the same engine; "WAL before stores" is the protocol's
//! ordering decision (step C). The snapshot root directory itself is
//! created atomically by the protocol (step A), never here.

use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::progress::CancelFlag;

/// Fixed reused buffer for streaming copies and hashes.
const STREAM_BUFFER_SIZE: usize = 1024 * 1024;

/// Bounded retry knobs for Windows sharing violations (os errors 32/33).
#[derive(Debug, Clone)]
pub struct CopyOptions {
    pub share_retry_attempts: u32,
    pub share_retry_delay_ms: u64,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            share_retry_attempts: 5,
            share_retry_delay_ms: 100,
        }
    }
}

/// One file of the snapshot: forward-slash path relative to the snapshot
/// root, lowercase-hex SHA-256, size in bytes.
#[derive(Debug, Clone)]
pub struct CopiedFile {
    pub rel_path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// What one copy pass defined. `files` IS the snapshot: only what the
/// final hash pass hashed (design 4.2 step D — the manifest never
/// describes a file set that no longer exists).
#[derive(Debug, Default)]
pub struct CopyPassResult {
    pub files: Vec<CopiedFile>,
    pub changed_files: u64,
    pub missing_files: u64,
}

/// Copy failures. Sharing violations exhausted after the bounded retry
/// surface as `Io` carrying the original error.
#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    #[error("backup copy cancelled")]
    Cancelled,
    #[error("copy failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn is_sharing_violation(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32) | Some(33))
}

fn hex_encode(digest: &[u8]) -> String {
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{:02x}", byte);
    }
    out
}

/// Test-only injection seams for the failure-injection suite (the
/// module body lives in `tests/copy_seams.rs`). Compiled ONLY under
/// `#[cfg(test)]` — no release build carries any of this code, and no
/// production behaviour reads it. Each hook is a scoped thread-local
/// cleared when its guard drops, so no state can leak between tests;
/// the protocol runs on the caller's thread, so the hooks observe
/// exactly the copy/hash calls of the run under test.
#[cfg(test)]
#[path = "tests/copy_seams.rs"]
pub(crate) mod test_seams;

/// Stream-hash a file, returning lowercase-hex SHA-256 and size.
pub fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    #[cfg(test)]
    {
        if let Some(result) = test_seams::fire_hash_file(path) {
            return result;
        }
    }
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_BUFFER_SIZE];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        size += read as u64;
    }
    Ok((hex_encode(&hasher.finalize()), size))
}

/// Copy `source` to `dest` through the fixed buffer, hashing the bytes
/// written. Windows sharing violations are retried with a bounded delay.
fn copy_with_retry(
    source: &Path,
    dest: &Path,
    options: &CopyOptions,
) -> std::io::Result<(String, u64)> {
    let mut attempt = 0u32;
    loop {
        match copy_once(source, dest) {
            Ok(result) => return Ok(result),
            Err(error) if is_sharing_violation(&error) => {
                attempt += 1;
                if attempt >= options.share_retry_attempts {
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(options.share_retry_delay_ms));
            }
            Err(error) => return Err(error),
        }
    }
}

fn copy_once(source: &Path, dest: &Path) -> std::io::Result<(String, u64)> {
    let mut input = fs::File::open(source)?;
    let mut output = fs::File::create(dest)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_BUFFER_SIZE];
    let mut size = 0u64;
    loop {
        let read = input.read(&mut buf)?;
        if read == 0 {
            break;
        }
        output.write_all(&buf[..read])?;
        #[cfg(test)]
        test_seams::fire_after_first_write(source)?;
        hasher.update(&buf[..read]);
        size += read as u64;
    }
    output.sync_all()?;
    Ok((hex_encode(&hasher.finalize()), size))
}

fn fsync_dir_best_effort(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(file) = fs::File::open(dir) {
            let _ = file.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

fn is_regular_file(meta: &fs::Metadata) -> bool {
    meta.is_file()
}

/// File names skipped in every listing pass, on ALL OSes so the
/// snapshot manifest does not differ across platforms. Both are runtime
/// lock artifacts, not data (restore recreates them on first open):
/// `lock` is fjall's active-store lock artifact — the live engine holds
/// a range lock on it, so on Windows copying it can never succeed while
/// the server is up; `.shamir.lock` is the data-dir maintenance lock
/// (`maintenance_lock.rs`, `fs4` exclusive try-lock, PID inside) held
/// for the whole life of the live server, so the same Windows
/// sharing-violation applies and the file is process state, not data.
const SKIP_FILE_NAMES: &[&str] = &["lock", ".shamir.lock"];

/// Deterministic listing of every regular file under `source_root`
/// (sorted per directory), skipping top-level directories named in
/// `skip_top_level_dirs`, any non-regular entry, and files named in
/// [`SKIP_FILE_NAMES`] (`.shamir.lock` and `lock` — runtime lock
/// artifacts, see above).
fn list_files(source_root: &Path, skip_top_level_dirs: &[&str]) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![source_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        entries.sort();
        for entry in entries {
            let meta = match fs::symlink_metadata(&entry) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            if meta.is_dir() {
                let at_top = entry.parent() == Some(source_root);
                let name = entry
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if at_top && skip_top_level_dirs.contains(&name.as_str()) {
                    continue;
                }
                stack.push(entry);
            } else if is_regular_file(&meta) {
                let name = entry
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if SKIP_FILE_NAMES.contains(&name.as_str()) {
                    continue;
                }
                out.push(entry);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// One copy pass `source_root` -> `dest_root` (design 4.2 steps C/D/E):
/// deterministic listing, per-file copy with bounded sharing-violation
/// retry, then a second hash pass over every SOURCE file that defines the
/// snapshot. A source file changed between copy and hash is re-copied up
/// to twice more; one still mismatching fails the pass. A source file
/// deleted mid-copy is excluded from the result. Files named in
/// [`SKIP_FILE_NAMES`] (`.shamir.lock`, `lock`) and files under any of
/// `exclude_dirs` (absolute source paths) are left out entirely.
pub fn run_copy_pass(
    source_root: &Path,
    dest_root: &Path,
    skip_top_level_dirs: &[&str],
    exclude_dirs: &[PathBuf],
    cancel: &CancelFlag,
    options: &CopyOptions,
) -> Result<CopyPassResult, CopyError> {
    let listing = list_files(source_root, skip_top_level_dirs).map_err(|error| CopyError::Io {
        path: source_root.to_path_buf(),
        source: error,
    })?;
    let mut copied: Vec<(String, PathBuf, PathBuf, String, u64)> = Vec::new();
    let mut changed_files = 0u64;
    let mut missing_files = 0u64;
    let mut synced_dirs: Vec<PathBuf> = Vec::new();
    for source_path in listing {
        if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        if exclude_dirs.iter().any(|dir| source_path.starts_with(dir)) {
            continue;
        }
        let rel = match source_path.strip_prefix(source_root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        let rel_slash = rel
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let dest_path = dest_root.join(rel);
        if let Some(parent) = dest_path.parent() {
            if fs::create_dir_all(parent).is_ok() && !synced_dirs.contains(&parent.to_path_buf()) {
                synced_dirs.push(parent.to_path_buf());
            }
        }
        let copy = match copy_with_retry(&source_path, &dest_path, options) {
            Ok(copy) => copy,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing_files += 1;
                continue;
            }
            Err(error) => {
                return Err(CopyError::Io {
                    path: source_path,
                    source: error,
                });
            }
        };
        #[cfg(test)]
        test_seams::fire_after_copy(&source_path);
        copied.push((rel_slash, source_path, dest_path, copy.0, copy.1));
    }
    for dir in &synced_dirs {
        fsync_dir_best_effort(dir);
    }
    fsync_dir_best_effort(dest_root);

    // Second hash pass: defines the snapshot (design 4.2 step D).
    let mut files = Vec::with_capacity(copied.len());
    for (rel_slash, source_path, dest_path, copy_hash, copy_size) in copied {
        if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let (pass_hash, pass_size) = match hash_file(&source_path) {
            Ok(result) => result,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing_files += 1;
                continue;
            }
            Err(error) => {
                return Err(CopyError::Io {
                    path: source_path,
                    source: error,
                });
            }
        };
        let (hash, size) = if pass_hash == copy_hash && pass_size == copy_size {
            (pass_hash, pass_size)
        } else {
            // Changed between copy and hash: re-copy, bounded (design 4.2 D).
            changed_files += 1;
            let mut hash = pass_hash;
            let mut size = pass_size;
            for _ in 0..2 {
                let recopy = match copy_with_retry(&source_path, &dest_path, options) {
                    Ok(result) => result,
                    Err(error) => {
                        return Err(CopyError::Io {
                            path: source_path,
                            source: error,
                        });
                    }
                };
                let rehash = match hash_file(&source_path) {
                    Ok(result) => result,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Err(CopyError::Io {
                            path: source_path,
                            source: std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "source vanished during re-copy",
                            ),
                        });
                    }
                    Err(error) => {
                        return Err(CopyError::Io {
                            path: source_path,
                            source: error,
                        });
                    }
                };
                let matches_copy = rehash.0 == recopy.0 && rehash.1 == recopy.1;
                hash = rehash.0;
                size = rehash.1;
                if matches_copy {
                    break;
                }
            }
            let _ = (&hash, &size);
            (hash, size)
        };
        files.push(CopiedFile {
            rel_path: rel_slash,
            sha256: hash,
            size_bytes: size,
        });
    }
    Ok(CopyPassResult {
        files,
        changed_files,
        missing_files,
    })
}
