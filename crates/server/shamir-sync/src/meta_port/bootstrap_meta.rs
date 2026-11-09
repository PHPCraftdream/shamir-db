use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::user_directory::FjallUserDirectory;
use shamir_connect::common::error::EnvironmentError;
use shamir_connect::common::time::UnixNanos;
use thiserror::Error;

/// Errors returned by `ServerMetaStore`.
#[derive(Debug, Error)]
pub enum MetaError {
    /// Operating system entropy or clock failure.
    #[error("environment: {0}")]
    Environment(#[from] EnvironmentError),
    /// fjall keyspace open / get / insert / persist failures.
    #[error("fjall: {0}")]
    Fjall(#[from] fjall::Error),
    /// MessagePack encode / decode failure or length mismatch on a fixed-size
    /// byte field.
    #[error("encoding: {0}")]
    Encoding(String),
    /// Plain I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A required key was absent — the store has not been initialised.
    #[error("not initialised: {0}")]
    NotInitialised(&'static str),
}

/// Read-only snapshot of the three fields the fail-closed bootstrap-TTL
/// security gate (`connection::handshake::run_handshake`, F-25/F-38) needs
/// to decide whether to reject a login for an outstanding, expired
/// bootstrap credential.
///
/// Unlike the three granular `bootstrap_*` getters — each of which
/// independently calls `read_blob(KEY_BOOTSTRAP)` and swallows any error
/// via `.ok()` (failing OPEN) — this snapshot is derived from a SINGLE
/// `ServerMetaStore::read_blob` so the caller can — and must — propagate
/// a genuine metadata read/decode failure as `Err` and fail CLOSED
/// (F-38, #846, P0).
///
/// Field derivation mirrors the granular getters exactly (kept in lockstep
/// so the security gate's `Ok` arm matches the pre-F-38 behaviour for the
/// readable cases):
/// - `active`: `bootstrap_token_hash.is_some()` (== `bootstrap_token_active`).
/// - `username`: `bootstrap_username` verbatim.
/// - `expired`: hash present AND `bootstrap_token_expires_at_ns <= now_ns`
///   (== `bootstrap_token_expired`).
///
/// This is LOCAL to `server_meta.rs`'s live-server bootstrap bookkeeping.
/// It is deliberately NOT the `shamir_connect::server::bootstrap::BootstrapState`
/// imported above — that is a heavier, `Mutex`-based type for the wire
/// challenge-response bootstrap flow whose own module doc states it is
/// "NOT wired into any live dispatch path" (dead code).
#[derive(Debug, Clone)]
pub struct BootstrapTokenSnapshot {
    /// A bootstrap-token hash is currently present (a token is outstanding).
    pub active: bool,
    /// Username the outstanding token was issued for, if any.
    pub username: Option<String>,
    /// The outstanding token's TTL has already passed
    /// (`bootstrap_token_expires_at_ns <= now_ns`).
    pub expired: bool,
}

/// Bootstrap metadata operations needed by sync handshakes.
pub trait BootstrapMeta: Send + Sync {
    fn now_unix_nanos(&self) -> Result<u64, EnvironmentError> {
        UnixNanos::try_now().map(|now| now.as_u64())
    }

    fn read_bootstrap_token_state(&self, now_ns: u64) -> Result<BootstrapTokenSnapshot, MetaError>;
    fn bootstrap_token_active(&self) -> bool;
    fn bootstrap_username(&self) -> Option<String>;
    fn bootstrap_token_path(&self) -> Option<PathBuf>;
    fn set_bootstrap_token(
        &self,
        username: &str,
        token_hash: [u8; 32],
        expires_at_ns: u64,
        token_path: PathBuf,
    ) -> Result<(), MetaError>;
    fn consume_bootstrap_token(&self) -> Result<(), MetaError>;
    fn rotate_bootstrap_credential_to_random<'a>(
        &self,
        user_dir: &'a FjallUserDirectory,
        username: &'a str,
        kdf: shamir_connect::common::kdf_params::KdfParams,
        now_ns: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
}
