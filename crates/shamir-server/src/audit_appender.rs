//! Durable audit appender — HMAC-chained tab-separated log + fjall checkpoint.
//!
//! Spec IMPL §3.3 NORMATIVE — two storage layers:
//!
//! 1. **Fixed-format log** (`data_dir/audit.log`) — one line per [`AuditEntry`],
//!    fields TAB-separated in fixed order; byte fields as base64url. Append-only.
//! 2. **Checkpoint fjall** (`data_dir/audit_checkpoint`) — single-row
//!    `last_audit_hmac` keyspace with `(next_seq: u64, prev_hmac: [u8; 32])`.
//!    Always committed with `db.persist(PersistMode::SyncAll)` (fsync).
//!
//! ## fsync semantics
//!
//! - **Strict mode** ([`FjallAuditAppender::open_strict`]): every
//!   [`AuditAppender::append_entry`] writes + `sync_all`s the log file
//!   synchronously.
//! - **Batched mode** ([`FjallAuditAppender::open_batched`]): entries are
//!   buffered in memory and flushed by a background tokio task at the
//!   configured interval (≤5s per spec).
//!
//! Checkpoint writes are always durable regardless of mode.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use parking_lot::Mutex;
use shamir_connect::server::audit_chain::{AuditAppender, AuditEntry};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Filename of the fixed-format audit log inside `data_dir`.
const AUDIT_LOG_FILENAME: &str = "audit.log";
/// Directory name of the fjall checkpoint database inside `data_dir`.
const CHECKPOINT_DB_DIRNAME: &str = "audit_checkpoint";
/// fjall keyspace holding the single `(next_seq, prev_hmac)` row.
const CHECKPOINT_KEYSPACE: &str = "last_audit_hmac";
/// Single key used inside the checkpoint keyspace.
const CHECKPOINT_KEY: &[u8] = &[0u8];

/// Errors raised by [`FjallAuditAppender`] open + flush operations.
#[derive(Debug, thiserror::Error)]
pub enum AppenderError {
    /// Underlying I/O failure (file open, write, fsync).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// fjall error while opening / committing the checkpoint keyspace.
    #[error("fjall: {0}")]
    Fjall(#[from] fjall::Error),
}

/// Encode a free-text field for embedding in the tab-separated log line.
fn escape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str(r"\\"),
            '\t' => out.push_str(r"\t"),
            '\n' => out.push_str(r"\n"),
            c => out.push(c),
        }
    }
    out
}

/// Decode a free-text field that was produced by [`escape_field`].
fn unescape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(c) => {
                    out.push('\\');
                    out.push(c);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Serialise one [`AuditEntry`] to the fixed-format log line (without trailing `\n`).
fn entry_to_line(entry: &AuditEntry) -> Vec<u8> {
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        entry.seq,
        entry.ts_ns,
        escape_field(&entry.event),
        escape_field(&entry.transport),
        escape_field(&entry.user),
        escape_field(&entry.ip_subnet),
        URL_SAFE_NO_PAD.encode(entry.session_id_prefix),
        escape_field(&entry.result),
        URL_SAFE_NO_PAD.encode(&entry.details_canonical_msgpack),
        URL_SAFE_NO_PAD.encode(entry.prev_hmac),
        URL_SAFE_NO_PAD.encode(entry.hmac),
    );
    line.into_bytes()
}

/// Parse one fixed-format log line back into an [`AuditEntry`].
fn line_to_entry(line: &str) -> Result<AuditEntry, AppenderError> {
    let invalid = |msg: &'static str| {
        AppenderError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
    };

    let mut cols = line.splitn(11, '\t');
    let seq: u64 = cols
        .next()
        .ok_or_else(|| invalid("missing seq"))?
        .parse()
        .map_err(|_| invalid("seq not u64"))?;
    let ts_ns: u64 = cols
        .next()
        .ok_or_else(|| invalid("missing ts_ns"))?
        .parse()
        .map_err(|_| invalid("ts_ns not u64"))?;
    let event = unescape_field(cols.next().ok_or_else(|| invalid("missing event"))?);
    let transport = unescape_field(cols.next().ok_or_else(|| invalid("missing transport"))?);
    let user = unescape_field(cols.next().ok_or_else(|| invalid("missing user"))?);
    let ip_subnet = unescape_field(cols.next().ok_or_else(|| invalid("missing ip_subnet"))?);

    let session_id_prefix_v = URL_SAFE_NO_PAD
        .decode(
            cols.next()
                .ok_or_else(|| invalid("missing session_id_prefix"))?,
        )
        .map_err(|_| invalid("session_id_prefix: invalid base64url"))?;
    if session_id_prefix_v.len() != 8 {
        return Err(invalid("session_id_prefix not 8 bytes"));
    }
    let mut session_id_prefix = [0u8; 8];
    session_id_prefix.copy_from_slice(&session_id_prefix_v);

    let result = unescape_field(cols.next().ok_or_else(|| invalid("missing result"))?);

    let details = URL_SAFE_NO_PAD
        .decode(
            cols.next()
                .ok_or_else(|| invalid("missing details_canonical_msgpack"))?,
        )
        .map_err(|_| invalid("details_canonical_msgpack: invalid base64url"))?;

    let prev_hmac_v = URL_SAFE_NO_PAD
        .decode(cols.next().ok_or_else(|| invalid("missing prev_hmac"))?)
        .map_err(|_| invalid("prev_hmac: invalid base64url"))?;
    if prev_hmac_v.len() != 32 {
        return Err(invalid("prev_hmac not 32 bytes"));
    }
    let mut prev_hmac = [0u8; 32];
    prev_hmac.copy_from_slice(&prev_hmac_v);

    let hmac_v = URL_SAFE_NO_PAD
        .decode(cols.next().ok_or_else(|| invalid("missing hmac"))?)
        .map_err(|_| invalid("hmac: invalid base64url"))?;
    if hmac_v.len() != 32 {
        return Err(invalid("hmac not 32 bytes"));
    }
    let mut hmac = [0u8; 32];
    hmac.copy_from_slice(&hmac_v);

    Ok(AuditEntry {
        seq,
        ts_ns,
        event,
        transport,
        user,
        ip_subnet,
        session_id_prefix,
        result,
        details_canonical_msgpack: details,
        prev_hmac,
        hmac,
    })
}

/// Durability mode of the appender.
enum Durab {
    Strict,
    Batched {
        buffer: Mutex<Vec<AuditEntry>>,
        notify: Arc<Notify>,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
        task: Mutex<Option<JoinHandle<()>>>,
    },
}

/// Optional log-rotation policy for the fixed-format audit file.
#[derive(Debug)]
pub struct RotationPolicy {
    pub max_size_bytes: u64,
    pub current_size: std::sync::atomic::AtomicU64,
    /// Delete rotated files older than this many days. `0` disables the
    /// retention sweep (operator manages retention out-of-band). The
    /// sweep runs piggyback on each rotation — see
    /// [`FjallAuditAppender::sweep_retention`].
    pub retention_days: u32,
}

/// Durable [`AuditAppender`] backed by fixed-format tab-separated log + fjall checkpoint.
pub struct FjallAuditAppender {
    log_file: Mutex<File>,
    log_path: PathBuf,
    /// Open fjall database holding the checkpoint keyspace.
    checkpoint_db: Arc<Database>,
    /// Checkpoint keyspace (single-row table).
    checkpoint_ks: Keyspace,
    mode: Durab,
    rotation: Option<RotationPolicy>,
}

impl core::fmt::Debug for FjallAuditAppender {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FjallAuditAppender")
            .field("log_path", &self.log_path)
            .field(
                "mode",
                match &self.mode {
                    Durab::Strict => &"strict",
                    Durab::Batched { .. } => &"batched",
                },
            )
            .finish()
    }
}

impl FjallAuditAppender {
    fn paths(data_dir: &Path) -> (PathBuf, PathBuf) {
        (
            data_dir.join(AUDIT_LOG_FILENAME),
            data_dir.join(CHECKPOINT_DB_DIRNAME),
        )
    }

    fn open_log(path: &Path) -> std::io::Result<File> {
        OpenOptions::new().create(true).append(true).open(path)
    }

    fn open_db(path: &Path) -> Result<(Arc<Database>, Keyspace), AppenderError> {
        let db = Database::builder(path).open()?;
        let ks = db.keyspace(CHECKPOINT_KEYSPACE, KeyspaceCreateOptions::default)?;
        Ok((Arc::new(db), ks))
    }

    /// Open in **strict** mode — every entry is fsync'd synchronously.
    pub fn open_strict(data_dir: impl AsRef<Path>) -> Result<Arc<Self>, AppenderError> {
        Self::open_strict_with_rotation(data_dir, None, 0)
    }

    /// Same as [`Self::open_strict`] with optional rotation cap (in bytes).
    /// `retention_days` controls the age-based retention sweep that runs
    /// piggyback on each rotation (`0` disables it).
    pub fn open_strict_with_rotation(
        data_dir: impl AsRef<Path>,
        max_size_bytes: Option<u64>,
        retention_days: u32,
    ) -> Result<Arc<Self>, AppenderError> {
        let dir = data_dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let (log_path, db_path) = Self::paths(dir);
        let log_file = Self::open_log(&log_path)?;
        let initial_size = log_file.metadata().map(|m| m.len()).unwrap_or(0);
        let (checkpoint_db, checkpoint_ks) = Self::open_db(&db_path)?;
        Ok(Arc::new(Self {
            log_file: Mutex::new(log_file),
            log_path,
            checkpoint_db,
            checkpoint_ks,
            mode: Durab::Strict,
            rotation: max_size_bytes.map(|m| RotationPolicy {
                max_size_bytes: m,
                current_size: std::sync::atomic::AtomicU64::new(initial_size),
                retention_days,
            }),
        }))
    }

    /// Open in **batched** mode — entries buffered in memory, flushed every
    /// `flush_every` (≤5s per spec). Checkpoint writes remain immediate.
    pub fn open_batched(
        data_dir: impl AsRef<Path>,
        flush_every: Duration,
    ) -> Result<Arc<Self>, AppenderError> {
        Self::open_batched_with_rotation(data_dir, flush_every, None, 0)
    }

    /// Same as [`Self::open_batched`] with optional rotation cap (in bytes).
    /// `retention_days` controls the age-based retention sweep that runs
    /// piggyback on each rotation (`0` disables it).
    pub fn open_batched_with_rotation(
        data_dir: impl AsRef<Path>,
        flush_every: Duration,
        max_size_bytes: Option<u64>,
        retention_days: u32,
    ) -> Result<Arc<Self>, AppenderError> {
        let dir = data_dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let (log_path, db_path) = Self::paths(dir);
        let log_file = Self::open_log(&log_path)?;
        let initial_size = log_file.metadata().map(|m| m.len()).unwrap_or(0);
        let (checkpoint_db, checkpoint_ks) = Self::open_db(&db_path)?;

        let notify = Arc::new(Notify::new());
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let me = Arc::new(Self {
            log_file: Mutex::new(log_file),
            log_path,
            checkpoint_db,
            checkpoint_ks,
            mode: Durab::Batched {
                buffer: Mutex::new(Vec::new()),
                notify: notify.clone(),
                shutdown: shutdown.clone(),
                task: Mutex::new(None),
            },
            rotation: max_size_bytes.map(|m| RotationPolicy {
                max_size_bytes: m,
                current_size: std::sync::atomic::AtomicU64::new(initial_size),
                retention_days,
            }),
        });

        // Spawn the background flusher.
        let weak = Arc::downgrade(&me);
        let task = tokio::spawn(async move {
            loop {
                let sleep = tokio::time::sleep(flush_every);
                tokio::pin!(sleep);
                tokio::select! {
                    _ = &mut sleep => {}
                    _ = notify.notified() => {}
                }
                let Some(strong) = weak.upgrade() else {
                    break;
                };
                if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = tokio::task::block_in_place(|| strong.flush_buffer());
                    break;
                }
                let _ = tokio::task::block_in_place(|| strong.flush_buffer());
            }
        });

        if let Durab::Batched { task: t, .. } = &me.mode {
            *t.lock() = Some(task);
        }
        Ok(me)
    }

    /// Read the persisted checkpoint, if any.
    ///
    /// Returns `Ok(None)` for fresh deployments (keyspace empty).
    pub fn load_checkpoint(
        data_dir: impl AsRef<Path>,
    ) -> Result<Option<(u64, [u8; 32])>, AppenderError> {
        let (_log_path, db_path) = Self::paths(data_dir.as_ref());
        if !db_path.exists() {
            return Ok(None);
        }
        let db = Database::builder(&db_path).open()?;
        let ks = db.keyspace(CHECKPOINT_KEYSPACE, KeyspaceCreateOptions::default)?;
        let Some(value) = ks.get(CHECKPOINT_KEY)? else {
            return Ok(None);
        };
        let bytes: &[u8] = value.as_ref();
        if bytes.len() != 40 {
            return Ok(None);
        }
        let mut seq = [0u8; 8];
        let mut hmac = [0u8; 32];
        seq.copy_from_slice(&bytes[..8]);
        hmac.copy_from_slice(&bytes[8..]);
        Ok(Some((u64::from_be_bytes(seq), hmac)))
    }

    /// Re-read the on-disk fixed-format log into [`AuditEntry`]s for
    /// startup verification ([`AuditChain::verify_chain`]).
    pub fn read_log_for_verify(
        data_dir: impl AsRef<Path>,
    ) -> Result<Vec<AuditEntry>, AppenderError> {
        let (log_path, _db_path) = Self::paths(data_dir.as_ref());
        let dir = data_dir.as_ref();
        let active_name = log_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(AUDIT_LOG_FILENAME)
            .to_string();
        let prefix = format!("{active_name}.");

        let mut rotated: Vec<PathBuf> = if dir.exists() {
            std::fs::read_dir(dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with(&prefix))
                        .unwrap_or(false)
                })
                .collect()
        } else {
            Vec::new()
        };
        rotated.sort();

        let mut all_paths = rotated;
        if log_path.exists() {
            all_paths.push(log_path);
        }

        let mut out = Vec::new();
        for path in all_paths {
            let f = File::open(&path)?;
            let r = BufReader::new(f);
            for line in r.lines() {
                let line = line?;
                if line.is_empty() {
                    continue;
                }
                out.push(line_to_entry(&line)?);
            }
        }
        Ok(out)
    }

    /// Force flush buffered entries (batched mode) and fsync.
    pub async fn flush_now(&self) -> Result<(), AppenderError> {
        match &self.mode {
            Durab::Strict => Ok(()),
            Durab::Batched { notify, .. } => {
                self.flush_buffer()?;
                notify.notify_one();
                Ok(())
            }
        }
    }

    /// Shutdown the batched flusher, draining any buffered entries.
    pub async fn shutdown(&self) {
        if let Durab::Batched {
            shutdown,
            notify,
            task,
            ..
        } = &self.mode
        {
            shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            notify.notify_one();
            let handle = task.lock().take();
            if let Some(h) = handle {
                let _ = h.await;
            }
            let _ = self.flush_buffer();
        }
    }

    /// Drain in-memory buffer to disk + fsync. Cheap no-op in strict mode.
    fn flush_buffer(&self) -> Result<(), AppenderError> {
        let entries: Vec<AuditEntry> = match &self.mode {
            Durab::Strict => return Ok(()),
            Durab::Batched { buffer, .. } => std::mem::take(&mut *buffer.lock()),
        };
        if entries.is_empty() {
            return Ok(());
        }
        let mut f = self.log_file.lock();
        let mut bytes_written: u64 = 0;
        for e in &entries {
            let mut bytes = entry_to_line(e);
            bytes.push(b'\n');
            f.write_all(&bytes)?;
            bytes_written += bytes.len() as u64;
        }
        f.flush()?;
        f.sync_all()?;
        if let Some(rot) = &self.rotation {
            let new_size = rot
                .current_size
                .fetch_add(bytes_written, std::sync::atomic::Ordering::Relaxed)
                + bytes_written;
            if new_size >= rot.max_size_bytes {
                if let Err(e) = self.rotate_locked(&mut f) {
                    tracing::warn!(error = %e, "audit log rotation failed");
                }
            }
        }
        Ok(())
    }

    /// Write one entry synchronously + fsync. Used in strict mode.
    fn append_strict(&self, entry: &AuditEntry) -> Result<(), AppenderError> {
        let mut bytes = entry_to_line(entry);
        bytes.push(b'\n');
        let bytes_written = bytes.len() as u64;
        let mut f = self.log_file.lock();
        f.write_all(&bytes)?;
        f.flush()?;
        f.sync_all()?;
        if let Some(rot) = &self.rotation {
            let new_size = rot
                .current_size
                .fetch_add(bytes_written, std::sync::atomic::Ordering::Relaxed)
                + bytes_written;
            if new_size >= rot.max_size_bytes {
                if let Err(e) = self.rotate_locked(&mut f) {
                    tracing::warn!(error = %e, "audit log rotation failed");
                }
            }
        }
        Ok(())
    }

    /// Rotate the current log file: fsync close, rename to
    /// `audit_log.log.<unix_nanos>`, open a fresh file. Caller MUST
    /// hold the `log_file` mutex (the `&mut File` parameter is the lock
    /// guard's deref).
    fn rotate_locked(&self, current: &mut File) -> Result<(), AppenderError> {
        current.flush()?;
        current.sync_all()?;
        let ts_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut rotated = self.log_path.clone();
        let stem = rotated
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audit_log.log".to_string());
        rotated.set_file_name(format!("{stem}.{ts_ns:020}"));
        std::fs::rename(&self.log_path, &rotated)?;
        *current = Self::open_log(&self.log_path)?;
        if let Some(rot) = &self.rotation {
            rot.current_size
                .store(0, std::sync::atomic::Ordering::Relaxed);
        }
        tracing::info!(rotated = %rotated.display(), "audit log rotated");

        // Best-effort retention sweep — runs piggyback on each rotation.
        // Only rotated files are candidates; the active log is never swept.
        // All errors are swallowed (logged at `warn`): retention is
        // housekeeping, not a correctness requirement — the write that
        // triggered this rotation has already succeeded.
        if let Some(rot) = &self.rotation {
            if rot.retention_days != 0 {
                self.sweep_retention(rot.retention_days);
            }
        }
        Ok(())
    }

    /// Delete rotated audit files whose embedded timestamp (or, as
    /// fallback, file mtime) is older than `retention_days * 86400 s`.
    ///
    /// Called from [`Self::rotate_locked`] after a successful rename +
    /// reopen. **Never fails** — every error (directory read failure,
    /// individual file deletion failure) is logged at `warn` and
    /// swallowed so the write path is unaffected.
    ///
    /// Age is determined from the `unix_nanos` suffix embedded in the
    /// filename (the same format [`Self::rotate_locked`] produces:
    /// `{stem}.<020d-nanos>`), avoiding reliance on filesystem mtime
    /// which can be wrong after a backup/restore/copy. Files whose
    /// suffix does not parse as nanos fall back to mtime; files from
    /// the future (clock skew) are never deleted.
    fn sweep_retention(&self, retention_days: u32) {
        const NANOS_PER_SEC: u64 = 1_000_000_000;
        const SECS_PER_DAY: u64 = 86400;
        let max_age_ns = (retention_days as u64)
            .saturating_mul(SECS_PER_DAY)
            .saturating_mul(NANOS_PER_SEC);
        let now_ns = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as u64,
            Err(_) => return, // clock before epoch — give up silently
        };
        let Some(stem) = self.log_path.file_name().and_then(|n| n.to_str()) else {
            return;
        };
        let prefix = format!("{stem}.");
        let Some(dir) = self.log_path.parent() else {
            return;
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "audit retention sweep: failed to read directory");
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // Only consider files matching the rotated-file naming pattern.
            let Some(suffix) = fname.strip_prefix(&prefix) else {
                continue;
            };
            // Determine the file's age: prefer the embedded nanos suffix,
            // fall back to file mtime if the suffix doesn't parse.
            let file_ns = suffix.parse::<u64>().ok().or_else(|| {
                entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
            });
            let Some(file_ns) = file_ns else {
                continue;
            };
            // Skip files from the future (clock skew) — never delete.
            let Some(age_ns) = now_ns.checked_sub(file_ns) else {
                continue;
            };
            if age_ns < max_age_ns {
                continue;
            }
            let age_days = age_ns / (SECS_PER_DAY * NANOS_PER_SEC);
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    tracing::info!(
                        file = %path.display(),
                        age_days,
                        "audit retention sweep: deleted old rotated file"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        file = %path.display(),
                        "audit retention sweep: failed to delete old rotated file"
                    );
                }
            }
        }
    }

    /// Persist `(next_seq, prev_hmac)` to the checkpoint keyspace.
    fn write_checkpoint(&self, next_seq: u64, prev_hmac: &[u8; 32]) -> Result<(), AppenderError> {
        let mut packed = [0u8; 40];
        packed[..8].copy_from_slice(&next_seq.to_be_bytes());
        packed[8..].copy_from_slice(prev_hmac);
        self.checkpoint_ks.insert(CHECKPOINT_KEY, &packed[..])?;
        // Spec IMPL §3.3 / §6.2: fsync before the checkpoint is observable.
        self.checkpoint_db.persist(PersistMode::SyncAll)?;
        Ok(())
    }
}

// `AuditAppender` is defined in `shamir-connect` and `Arc` is from `std`,
// so the orphan rule forbids `impl AuditAppender for Arc<FjallAuditAppender>`.
// Instead we implement on `FjallAuditAppender` directly — when callers wrap
// the inner type in `Arc<dyn AuditAppender>` (as `AuditChainWriter::new`
// requires) the `Arc::new` -> `Arc<dyn Trait>` coercion uses this impl.
impl AuditAppender for FjallAuditAppender {
    fn append_entry(&self, entry: &AuditEntry) {
        match &self.mode {
            Durab::Strict => {
                if let Err(e) = self.append_strict(entry) {
                    tracing::error!(error = %e, "audit appender strict write failed");
                }
            }
            Durab::Batched { buffer, .. } => {
                buffer.lock().push(entry.clone());
            }
        }
    }

    fn checkpoint(&self, next_seq: u64, prev_hmac: &[u8; 32]) {
        if let Err(e) = self.flush_buffer() {
            tracing::error!(error = %e, "audit appender flush before checkpoint failed");
        }
        if let Err(e) = self.write_checkpoint(next_seq, prev_hmac) {
            tracing::error!(error = %e, "audit appender checkpoint write failed");
        }
    }
}
