//! Durable audit appender — HMAC-chained JSON-line log + redb checkpoint.
//!
//! Spec IMPL §3.3 NORMATIVE — two storage layers:
//!
//! 1. **JSON-line log** (`data_dir/audit.log`) — one line per [`AuditEntry`],
//!    fields encoded as base64url for byte arrays. Append-only.
//! 2. **Checkpoint redb** (`data_dir/audit_checkpoint.redb`) — single-row
//!    `last_audit_hmac` table with `(next_seq: u64, prev_hmac: [u8; 32])`.
//!    Always committed with [`Durability::Immediate`] (fsync on commit).
//!
//! ## fsync semantics
//!
//! - **Strict mode** ([`RedbAuditAppender::open_strict`]): every
//!   [`AuditAppender::append_entry`] writes + `sync_all`s the log file
//!   synchronously.
//! - **Batched mode** ([`RedbAuditAppender::open_batched`]): entries are
//!   buffered in memory and flushed by a background tokio task at the
//!   configured interval (≤5s per spec).
//!
//! Checkpoint writes are always durable regardless of mode.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use parking_lot::Mutex;
use redb::{Database, Durability, ReadableDatabase, TableDefinition};
use serde::{Deserialize, Serialize};
use shamir_connect::server::audit_chain::{AuditAppender, AuditEntry};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Filename of the JSON-line audit log inside `data_dir`.
const AUDIT_LOG_FILENAME: &str = "audit.log";
/// Filename of the redb checkpoint database inside `data_dir`.
const CHECKPOINT_DB_FILENAME: &str = "audit_checkpoint.redb";

/// redb table holding `(next_seq, prev_hmac)`. Single row keyed by `0u8`.
const CHECKPOINT_TABLE: TableDefinition<&[u8; 1], &[u8; 40]> =
    TableDefinition::new("last_audit_hmac");

/// Errors raised by [`RedbAuditAppender`] open + flush operations.
#[derive(Debug, thiserror::Error)]
pub enum AppenderError {
    /// Underlying I/O failure (file open, write, fsync).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// redb error while opening / committing the checkpoint table.
    #[error("redb: {0}")]
    Redb(#[from] redb::Error),
    /// JSON serialisation / deserialisation failure.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<redb::DatabaseError> for AppenderError {
    fn from(e: redb::DatabaseError) -> Self {
        AppenderError::Redb(e.into())
    }
}

impl From<redb::TransactionError> for AppenderError {
    fn from(e: redb::TransactionError) -> Self {
        AppenderError::Redb(e.into())
    }
}

impl From<redb::TableError> for AppenderError {
    fn from(e: redb::TableError) -> Self {
        AppenderError::Redb(e.into())
    }
}

impl From<redb::StorageError> for AppenderError {
    fn from(e: redb::StorageError) -> Self {
        AppenderError::Redb(e.into())
    }
}

impl From<redb::CommitError> for AppenderError {
    fn from(e: redb::CommitError) -> Self {
        AppenderError::Redb(e.into())
    }
}

/// Wire format for one audit entry on disk — bytes base64url-encoded.
///
/// Private; callers re-decode via [`RedbAuditAppender::read_log_for_verify`].
#[derive(Serialize, Deserialize)]
struct JsonEntry {
    seq: u64,
    ts_ns: u64,
    event: String,
    transport: String,
    user: String,
    ip_subnet: String,
    /// base64url-encoded 8-byte session_id_prefix.
    session_id_prefix: String,
    result: String,
    /// base64url-encoded canonical msgpack details bytes.
    details_canonical_msgpack: String,
    /// base64url-encoded 32-byte previous-entry HMAC.
    prev_hmac: String,
    /// base64url-encoded 32-byte HMAC of this entry.
    hmac: String,
}

impl JsonEntry {
    fn from_audit(entry: &AuditEntry) -> Self {
        Self {
            seq: entry.seq,
            ts_ns: entry.ts_ns,
            event: entry.event.clone(),
            transport: entry.transport.clone(),
            user: entry.user.clone(),
            ip_subnet: entry.ip_subnet.clone(),
            session_id_prefix: URL_SAFE_NO_PAD.encode(entry.session_id_prefix),
            result: entry.result.clone(),
            details_canonical_msgpack: URL_SAFE_NO_PAD.encode(&entry.details_canonical_msgpack),
            prev_hmac: URL_SAFE_NO_PAD.encode(entry.prev_hmac),
            hmac: URL_SAFE_NO_PAD.encode(entry.hmac),
        }
    }

    fn into_audit(self) -> Result<AuditEntry, AppenderError> {
        let session_id_prefix_v = URL_SAFE_NO_PAD
            .decode(self.session_id_prefix.as_bytes())
            .map_err(|e| {
                AppenderError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?;
        let details = URL_SAFE_NO_PAD
            .decode(self.details_canonical_msgpack.as_bytes())
            .map_err(|e| {
                AppenderError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?;
        let prev_hmac_v = URL_SAFE_NO_PAD
            .decode(self.prev_hmac.as_bytes())
            .map_err(|e| {
                AppenderError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?;
        let hmac_v = URL_SAFE_NO_PAD.decode(self.hmac.as_bytes()).map_err(|e| {
            AppenderError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;

        let mut session_id_prefix = [0u8; 8];
        if session_id_prefix_v.len() != 8 {
            return Err(AppenderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session_id_prefix not 8 bytes",
            )));
        }
        session_id_prefix.copy_from_slice(&session_id_prefix_v);

        let mut prev_hmac = [0u8; 32];
        if prev_hmac_v.len() != 32 {
            return Err(AppenderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "prev_hmac not 32 bytes",
            )));
        }
        prev_hmac.copy_from_slice(&prev_hmac_v);

        let mut hmac = [0u8; 32];
        if hmac_v.len() != 32 {
            return Err(AppenderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "hmac not 32 bytes",
            )));
        }
        hmac.copy_from_slice(&hmac_v);

        Ok(AuditEntry {
            seq: self.seq,
            ts_ns: self.ts_ns,
            event: self.event,
            transport: self.transport,
            user: self.user,
            ip_subnet: self.ip_subnet,
            session_id_prefix,
            result: self.result,
            details_canonical_msgpack: details,
            prev_hmac,
            hmac,
        })
    }
}

/// Durability mode of the appender.
enum Durab {
    /// Each `append_entry` writes + fsyncs synchronously.
    Strict,
    /// Entries are buffered; a background task flushes periodically.
    Batched {
        /// Pending entries to write on next flush.
        buffer: Mutex<Vec<AuditEntry>>,
        /// Wakes up the flusher (used by `flush_now` + `shutdown`).
        notify: Arc<Notify>,
        /// Set to `true` to stop the flusher.
        shutdown: Arc<std::sync::atomic::AtomicBool>,
        /// Handle to the background flusher task.
        task: Mutex<Option<JoinHandle<()>>>,
    },
}

/// Optional log-rotation policy for the JSON-line audit file.
///
/// When set, every successful append checks the running file size; once it
/// crosses `max_size_bytes` the current file is renamed to
/// `audit_log.jsonl.<unix_nanos>` and a fresh `audit_log.jsonl` is opened
/// for subsequent writes. The HMAC chain is **not** affected — each
/// rotated entry already carries its `prev_hmac`, so verification stitches
/// the rotated files back together by reading them in lexicographic order.
#[derive(Debug)]
pub struct RotationPolicy {
    pub max_size_bytes: u64,
    pub current_size: std::sync::atomic::AtomicU64,
}

/// Durable [`AuditAppender`] backed by JSON-line log + redb checkpoint.
pub struct RedbAuditAppender {
    /// Open append-handle to `audit.log`.
    log_file: Mutex<File>,
    /// Path to the JSON-line log (used by static helpers).
    log_path: PathBuf,
    /// Open redb database for the checkpoint table.
    checkpoint_db: Arc<Database>,
    /// Durability mode (strict or batched).
    mode: Durab,
    /// Optional rotation policy. `None` = unlimited file size (legacy).
    rotation: Option<RotationPolicy>,
}

impl core::fmt::Debug for RedbAuditAppender {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RedbAuditAppender")
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

impl RedbAuditAppender {
    fn paths(data_dir: &Path) -> (PathBuf, PathBuf) {
        (
            data_dir.join(AUDIT_LOG_FILENAME),
            data_dir.join(CHECKPOINT_DB_FILENAME),
        )
    }

    fn open_log(path: &Path) -> std::io::Result<File> {
        OpenOptions::new().create(true).append(true).open(path)
    }

    fn open_db(path: &Path) -> Result<Arc<Database>, AppenderError> {
        let db = Database::create(path)?;
        // Ensure the table exists.
        let mut txn = db.begin_write()?;
        txn.set_durability(Durability::Immediate).ok();
        {
            let _t = txn.open_table(CHECKPOINT_TABLE)?;
        }
        txn.commit()?;
        Ok(Arc::new(db))
    }

    /// Open in **strict** mode — every entry is fsync'd synchronously.
    pub fn open_strict(data_dir: impl AsRef<Path>) -> Result<Arc<Self>, AppenderError> {
        Self::open_strict_with_rotation(data_dir, None)
    }

    /// Same as [`Self::open_strict`] with optional rotation cap (in bytes).
    pub fn open_strict_with_rotation(
        data_dir: impl AsRef<Path>,
        max_size_bytes: Option<u64>,
    ) -> Result<Arc<Self>, AppenderError> {
        let dir = data_dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let (log_path, db_path) = Self::paths(dir);
        let log_file = Self::open_log(&log_path)?;
        let initial_size = log_file.metadata().map(|m| m.len()).unwrap_or(0);
        let checkpoint_db = Self::open_db(&db_path)?;
        Ok(Arc::new(Self {
            log_file: Mutex::new(log_file),
            log_path,
            checkpoint_db,
            mode: Durab::Strict,
            rotation: max_size_bytes.map(|m| RotationPolicy {
                max_size_bytes: m,
                current_size: std::sync::atomic::AtomicU64::new(initial_size),
            }),
        }))
    }

    /// Open in **batched** mode — entries buffered in memory, flushed every
    /// `flush_every` (≤5s per spec). Checkpoint writes remain immediate.
    pub fn open_batched(
        data_dir: impl AsRef<Path>,
        flush_every: Duration,
    ) -> Result<Arc<Self>, AppenderError> {
        Self::open_batched_with_rotation(data_dir, flush_every, None)
    }

    /// Same as [`Self::open_batched`] with optional rotation cap (in bytes).
    pub fn open_batched_with_rotation(
        data_dir: impl AsRef<Path>,
        flush_every: Duration,
        max_size_bytes: Option<u64>,
    ) -> Result<Arc<Self>, AppenderError> {
        let dir = data_dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let (log_path, db_path) = Self::paths(dir);
        let log_file = Self::open_log(&log_path)?;
        let initial_size = log_file.metadata().map(|m| m.len()).unwrap_or(0);
        let checkpoint_db = Self::open_db(&db_path)?;

        let notify = Arc::new(Notify::new());
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let me = Arc::new(Self {
            log_file: Mutex::new(log_file),
            log_path,
            checkpoint_db,
            mode: Durab::Batched {
                buffer: Mutex::new(Vec::new()),
                notify: notify.clone(),
                shutdown: shutdown.clone(),
                task: Mutex::new(None),
            },
            rotation: max_size_bytes.map(|m| RotationPolicy {
                max_size_bytes: m,
                current_size: std::sync::atomic::AtomicU64::new(initial_size),
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
    /// Returns `Ok(None)` for fresh deployments (table empty).
    pub fn load_checkpoint(
        data_dir: impl AsRef<Path>,
    ) -> Result<Option<(u64, [u8; 32])>, AppenderError> {
        let (_log_path, db_path) = Self::paths(data_dir.as_ref());
        if !db_path.exists() {
            return Ok(None);
        }
        let db = Database::open(&db_path)?;
        let txn = db.begin_read()?;
        let table = match txn.open_table(CHECKPOINT_TABLE) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let key: &[u8; 1] = &[0u8];
        let entry = table.get(key)?;
        let Some(v) = entry else {
            return Ok(None);
        };
        let bytes = v.value();
        let mut seq = [0u8; 8];
        let mut hmac = [0u8; 32];
        seq.copy_from_slice(&bytes[..8]);
        hmac.copy_from_slice(&bytes[8..]);
        Ok(Some((u64::from_be_bytes(seq), hmac)))
    }

    /// Re-read the on-disk JSON-line log into [`AuditEntry`]s for
    /// startup verification ([`AuditChain::verify_chain`]).
    pub fn read_log_for_verify(
        data_dir: impl AsRef<Path>,
    ) -> Result<Vec<AuditEntry>, AppenderError> {
        let (log_path, _db_path) = Self::paths(data_dir.as_ref());
        let dir = data_dir.as_ref();
        // Collect rotated files (`audit.log.<timestamp>`) AND the active
        // `audit.log` in chronological order (oldest → newest). Since the
        // rotated suffix is a zero-padded `unix_nanos` (20 digits), a
        // lexicographic sort matches chronological order. The active
        // file is read LAST because its entries are the most recent.
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
                let je: JsonEntry = serde_json::from_str(&line)?;
                out.push(je.into_audit()?);
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
            // Final defensive flush in case the task missed the buffer.
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
            let je = JsonEntry::from_audit(e);
            let mut bytes = serde_json::to_vec(&je)?;
            bytes.push(b'\n');
            f.write_all(&bytes)?;
            bytes_written += bytes.len() as u64;
        }
        f.flush()?;
        f.sync_all()?;
        // Rotation check — done while holding the file lock so two flushes
        // can't race past the threshold simultaneously.
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
        let je = JsonEntry::from_audit(entry);
        let mut bytes = serde_json::to_vec(&je)?;
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
    /// `audit_log.jsonl.<unix_nanos>`, open a fresh file. Caller MUST
    /// hold the `log_file` mutex (the `&mut File` parameter is the lock
    /// guard's deref).
    fn rotate_locked(&self, current: &mut File) -> Result<(), AppenderError> {
        // Final sync before renaming so no buffered bytes are lost.
        current.flush()?;
        current.sync_all()?;
        // Build rotated name with a wall-clock timestamp so files sort
        // chronologically when listed.
        let ts_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut rotated = self.log_path.clone();
        let stem = rotated
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audit_log.jsonl".to_string());
        rotated.set_file_name(format!("{stem}.{ts_ns:020}"));
        std::fs::rename(&self.log_path, &rotated)?;
        // Open a new empty file in place.
        *current = Self::open_log(&self.log_path)?;
        if let Some(rot) = &self.rotation {
            rot.current_size.store(0, std::sync::atomic::Ordering::Relaxed);
        }
        tracing::info!(rotated = %rotated.display(), "audit log rotated");
        Ok(())
    }

    /// Persist `(next_seq, prev_hmac)` to the checkpoint table.
    fn write_checkpoint(&self, next_seq: u64, prev_hmac: &[u8; 32]) -> Result<(), AppenderError> {
        let mut packed = [0u8; 40];
        packed[..8].copy_from_slice(&next_seq.to_be_bytes());
        packed[8..].copy_from_slice(prev_hmac);
        let mut txn = self.checkpoint_db.begin_write()?;
        txn.set_durability(Durability::Immediate).ok();
        {
            let mut table = txn.open_table(CHECKPOINT_TABLE)?;
            let key: &[u8; 1] = &[0u8];
            table.insert(key, &packed)?;
        }
        txn.commit()?;
        Ok(())
    }
}

// `AuditAppender` is defined in `shamir-connect` and `Arc` is from `std`,
// so the orphan rule forbids `impl AuditAppender for Arc<RedbAuditAppender>`.
// Instead we implement on `RedbAuditAppender` directly — when callers wrap
// the inner type in `Arc<dyn AuditAppender>` (as `AuditChainWriter::new`
// requires) the `Arc::new` -> `Arc<dyn Trait>` coercion uses this impl.
impl AuditAppender for RedbAuditAppender {
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
        // Per spec, ensure the log is durable BEFORE the checkpoint that
        // claims to be at `next_seq` — otherwise a crash between log write
        // and checkpoint commit could leave the checkpoint pointing past
        // a non-fsynced suffix.
        if let Err(e) = self.flush_buffer() {
            tracing::error!(error = %e, "audit appender flush before checkpoint failed");
        }
        if let Err(e) = self.write_checkpoint(next_seq, prev_hmac) {
            tracing::error!(error = %e, "audit appender checkpoint write failed");
        }
    }
}
