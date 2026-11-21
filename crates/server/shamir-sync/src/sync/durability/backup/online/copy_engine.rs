//! Share-tolerant recursive copy with a snapshot-defining hash pass
//! (design `online-backup.md` 4.2, steps C/D/E). WAL directories go
//! through the same engine; "WAL before stores" is the protocol's
//! ordering decision (step C). The snapshot root directory itself is
//! created atomically by the protocol (step A), never here.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::manifest::BACKUP_META_FILE_NAME;
use super::progress::CancelFlag;
use super::protocol::SealRecopyCap;
use crate::sync::durability::backup::MANIFEST_FILE_NAME;

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
/// surface as `Io` carrying the original error; a tripped seal gate
/// surfaces as typed `RecopyCap` (the protocol maps it to
/// `OnlineBackupFailure::RecopyCap` at `Sealing`).
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
    #[error("seal re-copy cap exceeded: {0}")]
    RecopyCap(String),
}

/// Cooperative seal-re-copy gate (design Q12): bounds ONE repo's seal
/// re-copy (WAL dir + store dir) by wall-clock deadline and cumulative
/// copied bytes. The gate travels through the whole gated pass —
/// listing, per-chunk writes, bounded sharing-retry sleeps (clamped to
/// the deadline), and around each directory fsync — so the cap is
/// enforced cooperatively DURING the work, not only at completion. It
/// cannot preempt a blocking syscall: the deadline is a cooperative
/// bound checked at every engine loop point, never a detached kill.
/// `bytes` accumulates every byte actually written to the destination,
/// including the bytes of sharing-retry re-attempts and of the
/// hash-mismatch re-copies, across BOTH of the repo's gated passes
/// (WAL + store share one gate).
#[derive(Debug)]
pub(crate) struct SealGate {
    deadline: Option<Instant>,
    max_bytes: Option<u64>,
    bytes: Cell<u64>,
}

impl SealGate {
    /// Gate for one repo's seal re-copy; `started` is the per-repo
    /// monotonic instant taken immediately BEFORE the seal barrier, so
    /// the barrier's own latency is inside the budget. A configured
    /// clock cap beyond the platform's instant range is a typed refusal
    /// — never a silent unlimited clock.
    pub(crate) fn new(cap: &SealRecopyCap, started: Instant) -> Result<Self, CopyError> {
        let deadline = match cap.max_seconds {
            Some(seconds) => match started.checked_add(Duration::from_secs(seconds)) {
                Some(deadline) => Some(deadline),
                None => {
                    return Err(CopyError::RecopyCap(format!(
                        "configured clock cap {seconds}s overflows the platform clock"
                    )));
                }
            },
            None => None,
        };
        Ok(Self {
            deadline,
            max_bytes: cap.max_bytes,
            bytes: Cell::new(0),
        })
    }

    /// The gate's clock boundary, handed to the port's
    /// `seal_point_before` so an async server seal honours the same
    /// deadline.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(crate) fn check_clock(&self) -> Result<(), CopyError> {
        if self.deadline.is_some_and(|deadline| gate_now() >= deadline) {
            return Err(CopyError::RecopyCap(
                "seal re-copy exceeded its wall-clock deadline".to_string(),
            ));
        }
        Ok(())
    }

    /// Cooperative check at an engine loop point: cancellation first,
    /// then the clock.
    pub(crate) fn check(&self, cancel: &CancelFlag) -> Result<(), CopyError> {
        if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        self.check_clock()
    }

    /// Checked BEFORE writing each chunk: the clock, then the byte cap
    /// against the cumulative counter plus this chunk (saturating — a
    /// huge chunk cannot wrap past the cap).
    fn before_write(&self, chunk: u64) -> Result<(), CopyError> {
        self.check_clock()?;
        if let Some(max) = self.max_bytes {
            let written = self.bytes.get();
            // Saturating subtraction: correct even at max = u64::MAX.
            if chunk > max.saturating_sub(written) {
                return Err(CopyError::RecopyCap(format!(
                    "repo seal re-copy reached {written}+{chunk} bytes, cap {max}"
                )));
            }
        }
        Ok(())
    }

    /// Charged BEFORE the write lands: a partial or failed write keeps
    /// the charge (conservative — bytes that reached the destination
    /// file, and bytes a retry will re-attempt, both count against the
    /// cap). Saturating.
    fn charge(&self, chunk: u64) {
        self.bytes.set(self.bytes.get().saturating_add(chunk));
    }

    /// Sharing-retry sleep clamped to the deadline: never oversleeps
    /// past the gate, and refuses to sleep at all once past it.
    fn sleep(&self, delay: Duration) -> Result<(), CopyError> {
        match self.deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(gate_now());
                if remaining.is_zero() {
                    return self.check_clock();
                }
                std::thread::sleep(delay.min(remaining));
            }
            None => std::thread::sleep(delay),
        }
        Ok(())
    }
}

fn is_sharing_violation(error: &CopyError) -> bool {
    matches!(error, CopyError::Io { source, .. } if matches!(source.raw_os_error(), Some(32) | Some(33)))
}

/// The gate's monotonic now: the test clock seam under `cfg(test)`
/// (registered via `test_seams::set_monotonic_now`), the real clock
/// otherwise. Guarded by cfg attributes on statements, not `cfg!()` —
/// the seam module would fail to typecheck in a release build.
fn gate_now() -> Instant {
    #[cfg(test)]
    let now = test_seams::fire_monotonic_now(Instant::now());
    #[cfg(not(test))]
    let now = Instant::now();
    now
}

/// Every engine I/O failure is typed `CopyError::Io` naming the path it
/// was operating on — never string-erased through `io::Error::other`.
fn io_at(path: &Path, error: std::io::Error) -> CopyError {
    CopyError::Io {
        path: path.to_path_buf(),
        source: error,
    }
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
    hash_file_inner(path, &CancelFlag::default(), None).map_err(|error| match error {
        CopyError::Io { source, .. } => source,
        // Unreachable without a gate; kept typed rather than
        // string-erased.
        other => std::io::Error::other(other.to_string()),
    })
}

fn hash_file_inner(
    path: &Path,
    cancel: &CancelFlag,
    gate: Option<&SealGate>,
) -> Result<(String, u64), CopyError> {
    #[cfg(test)]
    {
        if let Some(result) = test_seams::fire_hash_file(path) {
            return result.map_err(|error| io_at(path, error));
        }
    }
    let mut file = fs::File::open(path).map_err(|error| io_at(path, error))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_BUFFER_SIZE];
    let mut size = 0u64;
    loop {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let read = file.read(&mut buf).map_err(|error| io_at(path, error))?;
        if read == 0 {
            // Clock and cancel checked on EOF too: the pass ends inside
            // the budget or not at all.
            if let Some(gate) = gate {
                gate.check(cancel)?;
            } else if cancel.is_cancelled() {
                return Err(CopyError::Cancelled);
            }
            break;
        }
        hasher.update(&buf[..read]);
        size += read as u64;
    }
    Ok((hex_encode(&hasher.finalize()), size))
}

/// Copy `source` to `dest` through the fixed buffer, hashing the bytes
/// written. Windows sharing violations are retried with a bounded
/// delay (clamped to the gate's deadline). Only `CopyError::Io`
/// sharing violations retry; caps and cancellation propagate typed.
fn copy_with_retry(
    source: &Path,
    dest: &Path,
    cancel: &CancelFlag,
    options: &CopyOptions,
    gate: Option<&SealGate>,
) -> Result<(String, u64), CopyError> {
    let mut attempt = 0u32;
    loop {
        match copy_once(source, dest, cancel, gate) {
            Ok(result) => return Ok(result),
            Err(error) if is_sharing_violation(&error) => {
                attempt += 1;
                if attempt >= options.share_retry_attempts {
                    return Err(error);
                }
                match gate {
                    Some(gate) => {
                        gate.sleep(Duration::from_millis(options.share_retry_delay_ms))?
                    }
                    None => std::thread::sleep(Duration::from_millis(options.share_retry_delay_ms)),
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn copy_once(
    source: &Path,
    dest: &Path,
    cancel: &CancelFlag,
    gate: Option<&SealGate>,
) -> Result<(String, u64), CopyError> {
    let mut input = fs::File::open(source).map_err(|error| io_at(source, error))?;
    let mut output = fs::File::create(dest).map_err(|error| io_at(source, error))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_BUFFER_SIZE];
    let mut size = 0u64;
    loop {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let read = input.read(&mut buf).map_err(|error| io_at(source, error))?;
        if read == 0 {
            // Clock and cancel checked on EOF too.
            if let Some(gate) = gate {
                gate.check(cancel)?;
            } else if cancel.is_cancelled() {
                return Err(CopyError::Cancelled);
            }
            break;
        }
        if let Some(gate) = gate {
            // Byte cap checked and CHARGED before the chunk lands: a
            // failed or partial write keeps the charge (conservative).
            gate.before_write(read as u64)?;
            gate.charge(read as u64);
        }
        output
            .write_all(&buf[..read])
            .map_err(|error| io_at(source, error))?;
        #[cfg(test)]
        test_seams::fire_after_first_write(source).map_err(|error| io_at(source, error))?;
        hasher.update(&buf[..read]);
        size += read as u64;
    }
    if let Some(gate) = gate {
        // Before and after the file fsync.
        gate.check(cancel)?;
    }
    output.sync_all().map_err(|error| io_at(source, error))?;
    if let Some(gate) = gate {
        gate.check(cancel)?;
    }
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
/// `skip_top_level_dirs`, top-level files named in `skip_top_level_files`,
/// any non-regular entry, and files named in [`SKIP_FILE_NAMES`]
/// (`.shamir.lock` and `lock` — runtime lock artifacts, see above). With
/// a gate, cancel and the clock are checked at every directory iteration
/// — the listing is inside the re-copy budget.
fn list_files(
    source_root: &Path,
    skip_top_level_dirs: &[&str],
    skip_top_level_files: &[&str],
    cancel: &CancelFlag,
    gate: Option<&SealGate>,
) -> Result<Vec<PathBuf>, CopyError> {
    let mut out = Vec::new();
    let mut stack = vec![source_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(|error| CopyError::Io {
                path: dir.clone(),
                source: error,
            })?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        entries.sort();
        for entry in entries {
            if let Some(gate) = gate {
                // Budget checked between individual directory entries.
                gate.check(cancel)?;
            } else if cancel.is_cancelled() {
                return Err(CopyError::Cancelled);
            }
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
                let Some(name) = entry.file_name() else {
                    continue;
                };
                if SKIP_FILE_NAMES
                    .iter()
                    .any(|candidate| name == OsStr::new(candidate))
                {
                    continue;
                }
                if !skip_top_level_files.is_empty()
                    && dir.as_path() == source_root
                    && skip_top_level_files
                        .iter()
                        .any(|candidate| name == OsStr::new(candidate))
                {
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
    run_copy_pass_inner(
        source_root,
        dest_root,
        &PassRules {
            skip_top_level_dirs,
            skip_top_level_files: &[],
            exclude_dirs,
            cancel,
            options,
            gate: None,
            preauth: &BTreeMap::new(),
        },
    )
}

pub(super) fn run_data_dir_copy_pass(
    source_root: &Path,
    dest_root: &Path,
    exclude_dirs: &[PathBuf],
    cancel: &CancelFlag,
    options: &CopyOptions,
) -> Result<CopyPassResult, CopyError> {
    run_copy_pass_inner(
        source_root,
        dest_root,
        &PassRules {
            skip_top_level_dirs: &["cache"],
            skip_top_level_files: &[MANIFEST_FILE_NAME, BACKUP_META_FILE_NAME],
            exclude_dirs,
            cancel,
            options,
            gate: None,
            preauth: &BTreeMap::new(),
        },
    )
}

/// Engine-internal pass rules shared by the copy loops (keeps argument
/// lists short).
struct PassRules<'a> {
    skip_top_level_dirs: &'a [&'a str],
    skip_top_level_files: &'a [&'a str],
    exclude_dirs: &'a [PathBuf],
    cancel: &'a CancelFlag,
    options: &'a CopyOptions,
    gate: Option<&'a SealGate>,
    preauth: &'a BTreeMap<String, (String, u64)>,
}

/// Same pass, GATED and INCREMENTAL (seal re-copy): `gate` bounds the
/// run cooperatively (listing, per-chunk writes, retry sleeps, fsync
/// surroundings, hash chunks) and switches the copy to incremental:
/// the first passes already copied the whole WAL/store, so for every
/// file the gate first compares the SOURCE against the ACTUAL
/// DESTINATION content by size + digest (never mtimes, never the
/// previous pass's returned hash, which may mismatch the destination
/// by design before self-verification) and skips the write when they
/// already agree; destination digests authenticated outside the hold
/// are reused by path. The gate is closed by the seal barrier for this
/// repo, so the compared source is stable; the snapshot-defining pass
/// still runs over authentic destination-backed content. Gated passes
/// never skip top-level dirs and never exclude (the protocol scopes
/// them per repo).
pub(crate) fn run_copy_pass_gated(
    source_root: &Path,
    dest_root: &Path,
    cancel: &CancelFlag,
    options: &CopyOptions,
    gate: &SealGate,
    preauth: &BTreeMap<String, (String, u64)>,
) -> Result<CopyPassResult, CopyError> {
    run_copy_pass_inner(
        source_root,
        dest_root,
        &PassRules {
            skip_top_level_dirs: &[],
            skip_top_level_files: &[],
            exclude_dirs: &[],
            cancel,
            options,
            gate: Some(gate),
            preauth,
        },
    )
}

/// Hash every file under `dest_root` (same listing rules as a copy
/// pass) into rel-path -> (digest, size). Run OUTSIDE the seal hold by
/// the protocol so the gated re-copy never re-reads unchanged
/// destinations under the hold; the map is a digest/size authentication
/// of ACTUAL destination content — never mtimes. Clock/cancel bounded
/// (a gate may be passed: reads count against the clock, not the byte
/// cap).
pub(crate) fn authenticate_dest(
    dest_root: &Path,
    cancel: &CancelFlag,
    gate: Option<&SealGate>,
) -> Result<BTreeMap<String, (String, u64)>, CopyError> {
    // A missing destination root authenticates as EMPTY: nothing is
    // proven about any file, so every file goes through hash-under-hold
    // (or a fresh copy).
    if fs::symlink_metadata(dest_root)
        .map(|meta| !meta.is_dir())
        .unwrap_or(true)
    {
        return Ok(BTreeMap::new());
    }
    let listing = list_files(dest_root, &[], &[], cancel, gate)?;
    let mut out = BTreeMap::new();
    for path in listing {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let rel = match path.strip_prefix(dest_root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        let rel_slash = rel
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let (digest, size) = hash_file_inner(&path, cancel, gate)?;
        out.insert(rel_slash, (digest, size));
    }
    Ok(out)
}

fn run_copy_pass_inner(
    source_root: &Path,
    dest_root: &Path,
    rules: &PassRules<'_>,
) -> Result<CopyPassResult, CopyError> {
    let listing = list_files(
        source_root,
        rules.skip_top_level_dirs,
        rules.skip_top_level_files,
        rules.cancel,
        rules.gate,
    )?;
    let &PassRules {
        exclude_dirs,
        cancel,
        options,
        gate,
        preauth,
        ..
    } = rules;
    // Last entry: the file's stored digest is already AUTHENTICATED
    // against the actual destination under the closed gate (source and
    // destination hashed equal), so the second hash pass must not
    // re-hash it — one comparison, no third hash of unchanged files.
    let mut copied: Vec<(String, PathBuf, PathBuf, String, u64, bool)> = Vec::new();
    let mut changed_files = 0u64;
    let mut missing_files = 0u64;
    let mut synced_dirs: Vec<PathBuf> = Vec::new();
    for source_path in listing {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
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
        // Incremental seal re-copy: under the closed gate, compare the
        // source against the ACTUAL destination content (size + digest;
        // authentic, never mtime-based, never the previous pass's
        // returned hash which may mismatch the destination by design
        // before self-verification). Destination digests authenticated
        // before the hold are reused by path; only a destination absent
        // from that map (new/changed file) is hashed under the hold.
        // Only changed or new files are written and hash-verified. A
        // source file already gone is missing, exactly as an uncopyable
        // file was before; caps and cancellation surface typed.
        if let Some(gate) = gate {
            let source_digest = match hash_file_inner(&source_path, cancel, Some(gate)) {
                Ok(digest) => digest,
                Err(CopyError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    missing_files += 1;
                    continue;
                }
                Err(other) => return Err(other),
            };
            let dest_digest = match preauth.get(&rel_slash) {
                Some(authenticated) => Some(authenticated.clone()),
                None => match hash_file_inner(&dest_path, cancel, Some(gate)) {
                    Ok(digest) => Some(digest),
                    Err(CopyError::Io { source, .. })
                        if source.kind() == std::io::ErrorKind::NotFound =>
                    {
                        None
                    }
                    Err(other) => return Err(other),
                },
            };
            if let Some(dest_digest) = dest_digest {
                if source_digest == dest_digest {
                    copied.push((
                        rel_slash,
                        source_path,
                        dest_path,
                        source_digest.0,
                        source_digest.1,
                        true,
                    ));
                    continue;
                }
            }
        }
        let copy = match copy_with_retry(&source_path, &dest_path, cancel, options, gate) {
            Ok(copy) => copy,
            Err(CopyError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                missing_files += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        #[cfg(test)]
        test_seams::fire_after_copy(&source_path);
        copied.push((rel_slash, source_path, dest_path, copy.0, copy.1, false));
    }
    for dir in &synced_dirs {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        }
        fsync_dir_best_effort(dir);
        if let Some(gate) = gate {
            gate.check(cancel)?;
        }
    }
    if let Some(gate) = gate {
        gate.check(cancel)?;
    }
    fsync_dir_best_effort(dest_root);
    if let Some(gate) = gate {
        gate.check(cancel)?;
    }

    // Second hash pass: defines the snapshot (design 4.2 step D) —
    // except for files whose stored digest was already authenticated
    // against the actual destination under the closed gate: there was
    // no copy race on that branch, and step-F self-verification still
    // re-hashes the actual destination.
    let mut files = Vec::with_capacity(copied.len());
    for (rel_slash, source_path, dest_path, copy_hash, copy_size, preverified) in copied {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let (hash, size) = if preverified {
            // Stored digest already authenticated against the actual
            // destination under the closed gate — no third hash.
            (copy_hash, copy_size)
        } else {
            let (pass_hash, pass_size) = match hash_file_inner(&source_path, cancel, gate) {
                Ok(result) => result,
                Err(CopyError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    missing_files += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if pass_hash == copy_hash && pass_size == copy_size {
                (pass_hash, pass_size)
            } else {
                // Changed between copy and hash: re-copy, bounded (design
                // 4.2 D).
                changed_files += 1;
                let mut hash = pass_hash;
                let mut size = pass_size;
                for _ in 0..2 {
                    let recopy = copy_with_retry(&source_path, &dest_path, cancel, options, gate)?;
                    let rehash = match hash_file_inner(&source_path, cancel, gate) {
                        Ok(result) => result,
                        Err(CopyError::Io { source, .. })
                            if source.kind() == std::io::ErrorKind::NotFound =>
                        {
                            return Err(CopyError::Io {
                                path: source_path,
                                source: std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    "source vanished during re-copy",
                                ),
                            });
                        }
                        Err(error) => return Err(error),
                    };
                    let matches_copy = rehash.0 == recopy.0 && rehash.1 == recopy.1;
                    hash = rehash.0;
                    size = rehash.1;
                    if matches_copy {
                        break;
                    }
                }
                (hash, size)
            }
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
