//! On-demand backup of the server's `data_dir` to a snapshot directory.
//!
//! v1: stop-and-copy. Operator stops the server, runs
//! `shamir-server --config <ktav> backup --to <dest>` which recursively
//! copies every file under `data_dir` (fjall database directories — each a
//! directory tree of journal and LSM segment files — plus wire-table
//! registries, audit log, rotated audit files, and TLS PEMs). The
//! destination is `<dest>/<timestamp>/` so a single `--to` can collect
//! daily snapshots. A `manifest.json` (file list + sha256 + size per file)
//! is written into the snapshot last, so `restore` (`restore.rs`) has
//! something durable to verify a snapshot against before trusting it.
//!
//! Why stop-and-copy instead of an online backup: fjall exposes no live /
//! online backup API on `Database` (its public surface is `insert`/`get`/
//! `range`/`persist`/`batch`; the closest analogue, `Snapshot`, is just a
//! seqno read snapshot, not a file-level backup). Copying the database
//! directory while the writer is paused (i.e. server stopped) is the
//! simplest path. Fjall is journal-based: every commit is appended to the
//! journal as a *batch* (`Start → items → End(xxh3 checksum)`), and the
//! default recovery mode (`RecoveryMode::TolerateCorruptTail`) discards a
//! corrupt/torn tail batch and truncates the journal back to the last
//! fully-checksummed batch boundary. So a copy taken during a quiescent
//! window between commits recovers, on next open, to that last complete
//! batch — a consistent pre-stop point. (A copy that races an in-flight
//! append merely loses the torn tail batch; it does not corrupt earlier
//! committed batches.) See `shamir-storage`'s `storage_fjall.rs` for how
//! `flush()` maps to `Database::persist(PersistMode::SyncAll)`.
//!
//! Future enhancement (P2): an in-process / online snapshot triggered by an
//! `admin` op, going through the engine's own repo handles (not this
//! offline CLI, which has no access to any live handle). Not done here
//! because it requires a wire-protocol change.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use shamir_collections::new_fx_set_wc;

/// Size of the fixed, reused buffer streaming hash reads are chunked
/// through — chosen so a large fjall SST/journal file is hashed without
/// ever materializing the whole file in RAM at once.
const HASH_STREAM_BUFFER_SIZE: usize = 1024 * 1024;

/// Stream `path`'s full contents through a fixed-size buffer into a running
/// SHA-256 hasher, returning `(digest, size_bytes)`. Produces a
/// byte-identical result to `sha256(&fs::read(path)?)`, but never holds more
/// than [`HASH_STREAM_BUFFER_SIZE`] bytes of file content in memory at once —
/// unlike `fs::read`, whose peak memory is proportional to the file size.
fn hash_file_streaming(path: &Path) -> std::io::Result<([u8; 32], u64)> {
    let mut file = fs::File::open(path)?;
    let mut buf = vec![0u8; HASH_STREAM_BUFFER_SIZE];
    let mut hasher = Sha256::new();
    let mut size_bytes = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size_bytes += n as u64;
    }
    Ok((hasher.finalize().into(), size_bytes))
}

/// Name of the manifest file written into every snapshot directory.
pub const MANIFEST_FILE_NAME: &str = "manifest.json";

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("backup io: {0}")]
    Io(#[from] std::io::Error),
    #[error("source dir does not exist: {0}")]
    SourceMissing(PathBuf),
    #[error("source is not a directory: {0}")]
    SourceNotDir(PathBuf),
    /// `verify_manifest`: `manifest.json` is absent from the snapshot dir.
    #[error("manifest missing: {0}")]
    ManifestMissing(PathBuf),
    /// `verify_manifest`: `manifest.json` exists but failed to parse.
    #[error("manifest invalid: {0}")]
    ManifestInvalid(String),
    /// `verify_manifest`: a file's actual sha256/size does not match what
    /// the manifest recorded — the snapshot must never be trusted/restored.
    #[error(
        "checksum mismatch for {path}: manifest says sha256={expected_sha256} \
         size={expected_size_bytes}, actual sha256={actual_sha256} size={actual_size_bytes}"
    )]
    ChecksumMismatch {
        path: PathBuf,
        expected_sha256: String,
        expected_size_bytes: u64,
        actual_sha256: String,
        actual_size_bytes: u64,
    },
    /// `verify_manifest`: a file is physically present under the snapshot
    /// directory but is not listed in the manifest — could indicate a
    /// tampered or hand-modified snapshot.
    #[error("file present on disk but not listed in manifest: {0}")]
    UnmanifestedFile(PathBuf),
    /// `verify_manifest`: `manifest.format_version` does not match the
    /// format this build understands — refuse rather than interpret
    /// entries under assumptions that may not hold for a different format.
    #[error("unsupported manifest format_version: found {found}, expected {expected}")]
    UnsupportedManifestFormatVersion { found: u32, expected: u32 },
    /// `verify_manifest`: an entry's `path` is absolute or contains a `..`
    /// (`ParentDir`) component — both can escape `snapshot_dir` when joined,
    /// so a tampered/malicious manifest could make `verify_manifest` read a
    /// file outside the snapshot directory.
    #[error("unsafe manifest entry path (absolute or contains '..'): {0}")]
    UnsafeManifestPath(String),
    /// `verify_manifest`: the same `path` appears more than once in
    /// `manifest.files` — a hand-crafted/tampered manifest could otherwise
    /// have one entry's checksum shadow another's expectations.
    #[error("duplicate manifest entry: {0}")]
    DuplicateManifestEntry(String),
    /// `backup()`: the destination (`to`, or its `<timestamp>` subdir) is
    /// inside (or equal to) the source `data_dir` — backing up would
    /// recursively copy the backup's own destination into itself.
    #[error(
        "backup destination is inside (or equal to) the source data_dir: \
         from={from} to={to}"
    )]
    DestinationInsideSource { from: PathBuf, to: PathBuf },
}

/// Outcome of a successful backup.
#[derive(Debug)]
pub struct BackupReport {
    /// The actual destination directory created (`<to>/<timestamp>`).
    pub dest_dir: PathBuf,
    /// Total bytes copied.
    pub bytes_copied: u64,
    /// Number of files copied.
    pub files_copied: u64,
    /// Path to the `manifest.json` written into `dest_dir`.
    pub manifest_path: PathBuf,
}

/// One entry in `manifest.json`'s `files` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFileEntry {
    /// Path relative to the snapshot directory, forward-slash separated
    /// (normalized even on Windows so a manifest written on one platform
    /// verifies correctly on another).
    pub path: String,
    /// Lowercase hex-encoded SHA-256 of the file's full contents.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// On-disk shape of `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub created_at_unix_ns: u128,
    pub files: Vec<ManifestFileEntry>,
}

/// Current manifest format version.
pub const MANIFEST_FORMAT_VERSION: u32 = 1;

/// Outcome of a successful [`verify_manifest`] call.
#[derive(Debug)]
pub struct ManifestVerifyReport {
    /// Number of files checked (equals `manifest.files.len()`).
    pub files_checked: u64,
    /// Sum of `size_bytes` across every checked file.
    pub total_bytes: u64,
}

/// Recursively copy `from` → `to/<timestamp>/`. The timestamp is
/// `YYYYMMDD_HHMMSS` in UTC (sortable, human-readable).
///
/// `to` is created if missing. The timestamped subdirectory MUST NOT
/// already exist (we refuse to overwrite — operator should pick a new
/// `--to`).
///
/// After the recursive copy completes, a `manifest.json` (see [`Manifest`])
/// is written into `dest_dir`, hashing every JUST-COPIED file with SHA-256.
/// This is an extra full read of every backed-up file — an acceptable,
/// honest cost for a correctness-critical manifest. `manifest.json` itself
/// is not included in its own `files` list (it is written after the list is
/// computed).
pub fn backup(from: &Path, to: &Path) -> Result<BackupReport, BackupError> {
    if !from.exists() {
        return Err(BackupError::SourceMissing(from.to_path_buf()));
    }
    if !from.is_dir() {
        return Err(BackupError::SourceNotDir(from.to_path_buf()));
    }
    reject_destination_inside_source(from, to)?;

    let stamp = utc_timestamp();
    let dest_dir = to.join(stamp);
    if dest_dir.exists() {
        return Err(BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", dest_dir.display()),
        )));
    }
    fs::create_dir_all(&dest_dir)?;

    let mut bytes = 0u64;
    let mut files = 0u64;
    copy_dir_recursive(from, &dest_dir, &mut bytes, &mut files)?;

    let manifest_path = write_manifest(&dest_dir)?;

    Ok(BackupReport {
        dest_dir,
        bytes_copied: bytes,
        files_copied: files,
        manifest_path,
    })
}

/// Reject `to` (the backup destination root) when it is inside, or equal
/// to, `from` (the source `data_dir`) — otherwise a
/// `backup --to <data_dir>/somewhere` (or `backup --to <data_dir>`) would
/// recursively back up the destination into itself, corrupting the
/// operation.
///
/// `to`'s own `<timestamp>` subdirectory does not exist yet at call time
/// (created later in [`backup`]), so this canonicalizes `to`'s nearest
/// EXISTING ancestor (walking up via `Path::parent` until something
/// resolves) rather than `to` itself, and compares that against `from`'s
/// canonical path.
fn reject_destination_inside_source(from: &Path, to: &Path) -> Result<(), BackupError> {
    let from_canonical = from.canonicalize().map_err(|e| {
        BackupError::Io(std::io::Error::new(
            e.kind(),
            format!("canonicalize source {}: {e}", from.display()),
        ))
    })?;

    let to_canonical = canonicalize_nearest_existing_ancestor(to)?;

    if to_canonical.starts_with(&from_canonical) {
        return Err(BackupError::DestinationInsideSource {
            from: from_canonical,
            to: to.to_path_buf(),
        });
    }
    Ok(())
}

/// Canonicalize `path` if it exists, else walk up `Path::parent` until an
/// existing ancestor is found and canonicalize that instead. `path` itself
/// is appended (non-canonicalized) onto the canonicalized ancestor so the
/// result still reflects `path`'s full intended location relative to that
/// ancestor.
fn canonicalize_nearest_existing_ancestor(path: &Path) -> Result<PathBuf, BackupError> {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    loop {
        if current.exists() {
            let mut canonical = current.canonicalize().map_err(|e| {
                BackupError::Io(std::io::Error::new(
                    e.kind(),
                    format!("canonicalize {}: {e}", current.display()),
                ))
            })?;
            for component in suffix.iter().rev() {
                canonical.push(component);
            }
            return Ok(canonical);
        }
        match current.parent() {
            Some(parent) => {
                if let Some(name) = current.file_name() {
                    suffix.push(name.to_os_string());
                }
                current = parent;
            }
            None => {
                // Reached the root without finding an existing ancestor —
                // treat the (non-canonicalized) original path as-is.
                return Ok(path.to_path_buf());
            }
        }
    }
}

/// Walk `dest_dir` (a just-written snapshot), hash every file, and write
/// `dest_dir/manifest.json`. Returns the manifest's path.
fn write_manifest(dest_dir: &Path) -> Result<PathBuf, BackupError> {
    let mut entries = Vec::new();
    collect_manifest_entries(dest_dir, dest_dir, &mut entries)?;

    let manifest = Manifest {
        format_version: MANIFEST_FORMAT_VERSION,
        created_at_unix_ns: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        files: entries,
    };

    let manifest_path = dest_dir.join(MANIFEST_FILE_NAME);
    let json = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| BackupError::ManifestInvalid(format!("encode: {e}")))?;
    fs::write(&manifest_path, json)?;
    Ok(manifest_path)
}

/// Recursively walk `dir` (rooted at `root`), hashing every regular file and
/// pushing a [`ManifestFileEntry`] with a `root`-relative, forward-slash
/// path. Skips `manifest.json` itself at the root (it does not exist yet at
/// call time, but the check is defensive against a future caller re-running
/// this over an already-manifested directory).
fn collect_manifest_entries(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<ManifestFileEntry>,
) -> Result<(), BackupError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_manifest_entries(root, &path, entries)?;
        } else if ft.is_file() {
            let rel = relative_slash_path(root, &path);
            if rel == MANIFEST_FILE_NAME {
                continue;
            }
            let (digest, size_bytes) = hash_file_streaming(&path)?;
            entries.push(ManifestFileEntry {
                path: rel,
                sha256: hex::encode(digest),
                size_bytes,
            });
        }
        // Symlinks / sockets / fifos: `copy_dir_recursive` already skipped
        // these with a warning; nothing to hash here either.
    }
    Ok(())
}

/// `path` relative to `root`, forward-slash separated regardless of
/// platform (so a manifest written on Windows verifies correctly on Linux
/// and vice versa).
fn relative_slash_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Read `snapshot_dir/manifest.json`, re-hash every listed file, and
/// confirm no extra unmanifested file is physically present. A checksum /
/// size mismatch, a missing manifest, an unparseable manifest, or an extra
/// unmanifested file are all hard errors — a corrupted or tampered snapshot
/// must never be silently accepted.
pub fn verify_manifest(snapshot_dir: &Path) -> Result<ManifestVerifyReport, BackupError> {
    let manifest_path = snapshot_dir.join(MANIFEST_FILE_NAME);
    if !manifest_path.exists() {
        return Err(BackupError::ManifestMissing(manifest_path));
    }
    let raw = fs::read(&manifest_path)?;
    let manifest: Manifest = serde_json::from_slice(&raw).map_err(|e| {
        BackupError::ManifestInvalid(format!("decode {}: {e}", manifest_path.display()))
    })?;

    if manifest.format_version != MANIFEST_FORMAT_VERSION {
        return Err(BackupError::UnsupportedManifestFormatVersion {
            found: manifest.format_version,
            expected: MANIFEST_FORMAT_VERSION,
        });
    }

    let mut accounted: shamir_collections::TFxSet<String> = new_fx_set_wc(manifest.files.len());

    let mut total_bytes = 0u64;
    for entry in &manifest.files {
        reject_unsafe_manifest_path(&entry.path)?;
        if accounted.contains(&entry.path) {
            return Err(BackupError::DuplicateManifestEntry(entry.path.clone()));
        }

        let file_path = snapshot_dir.join(&entry.path);
        let (digest, actual_size) = hash_file_streaming(&file_path).map_err(|e| {
            BackupError::Io(std::io::Error::new(
                e.kind(),
                format!("manifest entry {}: {e}", entry.path),
            ))
        })?;
        let actual_sha256 = hex::encode(digest);

        if actual_size != entry.size_bytes || actual_sha256 != entry.sha256 {
            return Err(BackupError::ChecksumMismatch {
                path: file_path,
                expected_sha256: entry.sha256.clone(),
                expected_size_bytes: entry.size_bytes,
                actual_sha256,
                actual_size_bytes: actual_size,
            });
        }
        total_bytes += actual_size;
        accounted.insert(entry.path.clone());
    }

    // Every file physically present under snapshot_dir (except the
    // manifest itself) must be accounted for in the manifest.
    let mut on_disk = Vec::new();
    collect_relative_file_paths(snapshot_dir, snapshot_dir, &mut on_disk)?;
    for rel in on_disk {
        if rel == MANIFEST_FILE_NAME {
            continue;
        }
        if !accounted.contains(&rel) {
            return Err(BackupError::UnmanifestedFile(snapshot_dir.join(&rel)));
        }
    }

    Ok(ManifestVerifyReport {
        files_checked: manifest.files.len() as u64,
        total_bytes,
    })
}

/// Reject a manifest entry `path` that is absolute or contains a `..`
/// (`ParentDir`) component — either would let `snapshot_dir.join(path)`
/// escape `snapshot_dir` (an absolute `PathBuf::join` argument REPLACES the
/// base entirely; a relative `..` component walks back out of it), letting
/// a tampered/malicious manifest make [`verify_manifest`] read a file
/// outside the snapshot directory.
fn reject_unsafe_manifest_path(entry_path: &str) -> Result<(), BackupError> {
    let path = Path::new(entry_path);
    if path.is_absolute() {
        return Err(BackupError::UnsafeManifestPath(entry_path.to_string()));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(BackupError::UnsafeManifestPath(entry_path.to_string()));
    }
    Ok(())
}

/// Recursively collect every regular file under `dir` (rooted at `root`) as
/// a `root`-relative, forward-slash path. Used by [`verify_manifest`] to
/// detect extra unmanifested files.
fn collect_relative_file_paths(
    root: &Path,
    dir: &Path,
    out: &mut Vec<String>,
) -> Result<(), BackupError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_relative_file_paths(root, &path, out)?;
        } else if ft.is_file() {
            out.push(relative_slash_path(root, &path));
        }
    }
    Ok(())
}

/// `YYYYMMDD_HHMMSS` UTC. Pure-Rust without `chrono` to keep deps light.
pub(crate) fn utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{y:04}{mo:02}{d:02}_{h:02}{mi:02}{s:02}")
}

/// Convert a unix epoch second into (year, month, day, hour, minute, second)
/// in UTC. Algorithm: civil_from_days from Howard Hinnant's date library
/// (public domain). Adapted to Rust ints.
fn unix_to_ymd_hms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let s = (secs % 86_400) as u32;
    let h = s / 3600;
    let mi = (s % 3600) / 60;
    let se = s % 60;

    // Hinnant's civil_from_days, valid for any day in [-12687428, 11248737].
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = (if mo <= 2 { y + 1 } else { y }) as i32;
    (y, mo, d, h, mi, se)
}

/// `pub(crate)` — reused by `restore.rs` to copy a snapshot into a
/// temporary sibling directory before the atomic swap into `data_dir`.
pub(crate) fn copy_dir_recursive(
    src: &Path,
    dst: &Path,
    bytes: &mut u64,
    files: &mut u64,
) -> std::io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            fs::create_dir_all(&target)?;
            copy_dir_recursive(&path, &target, bytes, files)?;
        } else if ft.is_file() {
            let n = fs::copy(&path, &target)?;
            *bytes += n;
            *files += 1;
        } else {
            // Symlinks / sockets / fifos: skip with a warning. A fjall
            // database is a directory tree of journal (`*.jnl`) + LSM
            // segment files — none of those should normally be one of these.
            tracing::warn!(path = %path.display(), "backup: skipping non-regular file");
        }
    }
    Ok(())
}
