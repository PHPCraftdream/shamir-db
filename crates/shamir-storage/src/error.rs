use thiserror::Error;

use shamir_types::codecs::CodecError;

/// A generic error type for all database and storage operations.
#[derive(Error, Debug)]
pub enum DbError {
    /// The requested item was not found.
    #[error("Item not found: {0}")]
    NotFound(String),

    /// The key already exists and cannot be inserted again.
    #[error("Key already exists: {0}")]
    KeyExists(String),

    /// Duplicate value for unique index.
    #[error("Duplicate key: {0}")]
    DuplicateKey(String),

    /// Cannot create unique index due to duplicate values.
    /// Contains: (index_name, duplicate_count, sample_value)
    #[error(
        "Cannot create unique index '{0}': found {1} records with duplicate values (example: {2})"
    )]
    UniqueIndexCreationFailed(String, usize, String),

    /// An error originating from the underlying storage backend.
    #[error("Storage backend error: {0}")]
    Storage(String),

    /// A WAL-less apply failed or was interrupted; new writes require reopen.
    #[error("Repository requires reopen: {0}")]
    RepoRequiresReopen(String),

    // VIS-4a: exceptional STRICT acknowledgement, not a retryable write failure.
    #[error(
        "Write committed at {commit_version}, visible through {visible_through}: {reason}; do not retry"
    )]
    WriteCommittedVisibilityPending {
        commit_version: u64,
        visible_through: u64,
        reason: &'static str,
    },

    /// An error related to configuration.
    #[error("Configuration error: {0}")]
    Config(String),

    /// An error during serialization or deserialization.
    #[error("Codec error: {0}")]
    Codec(String),

    /// A generic I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// An internal logic error.
    #[error("Internal error: {0}")]
    Internal(String),

    /// A validation error (e.g., schema validation).
    #[error("Validation error: {0}")]
    Validation(String),

    /// The requested snapshot version has not been allocated yet.
    #[error("AsOf version {requested} exceeds allocation high-water mark {allocated}")]
    AsOfFutureVersion { requested: u64, allocated: u64 },

    /// An indeterminate WAL outcome prevents the snapshot from settling.
    #[error("AsOf version {requested} cannot become visible; reopen the repository")]
    AsOfReopenRequired { requested: u64 },

    // VIS-2 read-only observation errors (separate from follower durability).
    #[error("Visibility version {requested} exceeds allocation high-water mark {allocated}")]
    VisibilityFutureVersion { requested: u64, allocated: u64 },

    #[error("Invalid visibility wait: {reason}")]
    VisibilityWaitInvalid { reason: String },

    #[error("AsOf version {requested} timed out at visible prefix {visible_through}")]
    AsOfVisibilityTimeout {
        requested: u64,
        visible_through: u64,
        blocking_version: u64,
    },

    /// An error from the function engine (compile, invoke, registry).
    #[error("Function error: {0}")]
    Function(String),

    /// One or more validators rejected the write with structured,
    /// field-bound errors (S3). The inner string is a semicolon-separated
    /// list of `"field.path: code"` entries (record-level errors omit the
    /// field path). Example: `"_prev_hash: stale; : required"`.
    #[error("Validator rejected: {0}")]
    ValidatorRejected(String),

    /// A validator could not be invoked (missing from registry, WASM
    /// trap, undecodable return). Fail-closed: operator/deploy fault.
    #[error("Validator invalid: {0}")]
    ValidatorInvalid(String),

    /// A transactional conflict aborted the operation. Carries a
    /// human-readable description of the conflict (e.g. wound-wait abort,
    /// lock contention that could not be resolved). The engine maps this
    /// to a `"tx_conflict"`-style wire reason where appropriate.
    #[error("Conflict: {0}")]
    Conflict(String),

    /// Optimistic-concurrency (CAS) mismatch on `UpdateOp`/`DeleteOp::
    /// expected_version`. The detail string names the offending table/key
    /// plus the expected and actual versions. Engine wire-mapping surfaces
    /// this as `BatchError::QueryError { code: Some("version_conflict") }`
    /// (and `DbResponse::Error { code: "version_conflict" }`); TS callers
    /// branch on `ShamirDbError.code === "version_conflict"`.
    #[error("Version conflict: {0}")]
    VersionConflict(String),

    /// Source writes are temporarily frozen for an online migration.
    #[error("table '{table}' is frozen for migration")]
    MigrationFrozen { table: String },

    /// An index is currently being dropped and is in its reader-drain window.
    /// The lookup result is deliberately withheld. Callers should retry or
    /// re-plan without this index.
    #[error(
        "Index '{0}' is currently being dropped; lookup result unavailable — retry or re-plan"
    )]
    IndexDrainInProgress(String),

    /// A repository-wide atomic batch (`Repo::begin_atomic_batch`) is not
    /// supported by this backend.
    #[error("Repository-wide atomic batch is not supported by this backend: {0}")]
    AtomicBatchUnsupported(String),

    /// S5c-X PR8b: an ordinary read hit a replication destination that is in
    /// `resync_required` quarantine (identity/epoch fence tripped or terminal
    /// journal gap). The destination's local state is stale/diverged; reads are
    /// refused until a replacement seed is installed and the quarantine cleared.
    ///
    /// Boxed: three inline `String`s made this the largest variant and grew
    /// every `DbResult` on every path (nested-batch stack overflow in CI).
    #[error(
        "repository {}/{} is quarantined (resync_required): {}",
        .0.db,
        .0.repo,
        .0.reason
    )]
    ResyncRequired(Box<ResyncRequiredInfo>),
}

/// Heap payload of [`DbError::ResyncRequired`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResyncRequiredInfo {
    pub db: String,
    pub repo: String,
    pub reason: String,
}

// Ratchet: a new inline-heavy variant must be boxed, not widen every `DbResult`.
const _: () = assert!(std::mem::size_of::<DbError>() <= 64);

impl From<CodecError> for DbError {
    fn from(err: CodecError) -> Self {
        DbError::Codec(err.to_string())
    }
}

impl DbError {
    /// Build [`DbError::ResyncRequired`] for a quarantined `db`/`repo`.
    pub fn resync_required(db: &str, repo: &str, reason: String) -> Self {
        DbError::ResyncRequired(Box::new(ResyncRequiredInfo {
            db: db.to_string(),
            repo: repo.to_string(),
            reason,
        }))
    }

    /// Return the machine-readable wire code for this error variant, if any.
    /// Used by the engine to populate `BatchError::QueryError.code` (and
    /// `DbResponse::Error.code` on the wire) so clients can branch on typed
    /// codes instead of regex-matching messages.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            DbError::WriteCommittedVisibilityPending { .. } => {
                Some("write_committed_visibility_pending")
            }
            DbError::VisibilityFutureVersion { .. } => Some("visibility_future_version"),
            DbError::VisibilityWaitInvalid { .. } => Some("visibility_wait_invalid"),
            DbError::AsOfVisibilityTimeout { .. } => Some("as_of_visibility_timeout"),
            DbError::RepoRequiresReopen(_) => Some("repo_requires_reopen"),
            DbError::AsOfFutureVersion { .. } => Some("as_of_future_version"),
            DbError::AsOfReopenRequired { .. } => Some("as_of_reopen_required"),
            DbError::MigrationFrozen { .. } => Some("table_migrating"),
            DbError::VersionConflict(_) => Some("version_conflict"),
            DbError::Conflict(_) => Some("tx_conflict"),
            DbError::IndexDrainInProgress(_) => Some("index_drain_in_progress"),
            DbError::ResyncRequired { .. } => Some("resync_required"),
            _ => None,
        }
    }
}

pub type DbResult<T> = Result<T, DbError>;
