//! The copy engine's API types: the retry knobs, one captured file, one
//! pass's result and the typed copy failures. Split out of `copy_engine.rs`
//! by the layout rule (at most 1000 lines per source file); `copy_engine`
//! re-exports every item, so the paths existing callers use
//! (`copy_engine::CopyError`, `copy_engine::CopyOptions`, ...) are unchanged.

use std::path::PathBuf;

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
/// surface as `Io` (source) or `Dest` (destination) carrying the
/// original error; a tripped seal gate surfaces as typed `RecopyCap`
/// (the protocol maps it to `OnlineBackupFailure::RecopyCap` at
/// `Sealing`). The bounded audit chain rounds of the data-directory pass
/// fail typed when an archive keeps appearing behind the copy.
///
/// `AuditFilesUnstable`, `AuditListing` and `AuditArchiveLost` are new public Rust surface:
/// `publish = false` only stops registry publishing, it does not make the API
/// private — sibling crates (`shamir-server`) can name and match these
/// variants. There is no semver obligation for them, and neither is a stable
/// error code: the stable, user-visible vocabulary is
/// `OnlineBackupError::stage_and_reason`.
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
    /// The DESTINATION side failed (create, write, fsync, parent
    /// directory): kept distinct from [`CopyError::Io`] so a destination
    /// error can never be mistaken for the tolerated vanished-SOURCE
    /// race that `Io { NotFound }` encodes.
    #[error("destination write failed for {path}: {source}")]
    Dest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("seal re-copy cap exceeded: {0}")]
    RecopyCap(String),
    #[error("audit chain files kept changing during the copy pass ({rounds} rounds)")]
    AuditFilesUnstable { rounds: usize },
    /// The destination run directory holds a reserved audit chain file
    /// (`audit.log` or an `audit.log.` archive) that the accepted
    /// (manifest-defining) round did not capture. An earlier pass or an earlier
    /// unstable round copied it and a rotation, or a retention sweep, then
    /// removed it from the source. Publishing either drops bytes a captured
    /// anchor may name or describes a file set the destination contradicts, so
    /// the pass fails closed BEFORE the manifest is written; nothing is deleted,
    /// the cleanup guard removes the whole run directory and the next scheduled
    /// run retries. No path and no archive name in the text.
    #[error("the destination holds an audit chain file the accepted round did not capture")]
    AuditArchiveLost,
    /// One entry of the audit chain directory could not be enumerated or
    /// examined: an entry of its listing, or that entry's metadata, failed
    /// with an error other than `NotFound`. Deliberately carries NO path —
    /// the audit listing reports a per-entry failure in the copy's
    /// path-free vocabulary (`copy-io-<kind>`), never with a filesystem
    /// path.
    #[error("audit chain directory entry could not be read: {source}")]
    AuditListing {
        #[source]
        source: std::io::Error,
    },
}
