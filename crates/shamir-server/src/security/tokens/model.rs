use super::store::TokenStoreError;
use zeroize::Zeroizing;

/// Secret-free public projection of a token record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenView {
    /// Token identifier.
    pub id: [u8; 16],
    /// Owning user identifier.
    pub owner_user_id: [u8; 16],
    /// User-selected label.
    pub label: String,
    /// Creation timestamp in Unix nanoseconds.
    pub created_at_ns: u64,
    /// Expiration timestamp in Unix nanoseconds.
    pub expires_at_ns: u64,
    /// Granted access mode.
    pub mode: shamir_connect::server::token::TokenMode,
    /// Allowed endpoint channels.
    pub channels: shamir_connect::server::token::TokenChannels,
    /// Optional database allowlist.
    pub databases: Option<Vec<String>>,
    /// Revocation timestamp, if revoked.
    pub revoked_at_ns: Option<u64>,
}

impl From<&shamir_connect::server::token::TokenRecord> for TokenView {
    fn from(record: &shamir_connect::server::token::TokenRecord) -> Self {
        Self {
            id: record.id,
            owner_user_id: record.owner_user_id,
            label: record.label.clone(),
            created_at_ns: record.created_at_ns,
            expires_at_ns: record.expires_at_ns,
            mode: record.mode,
            channels: record.channels,
            databases: record.databases.clone(),
            revoked_at_ns: record.revoked_at_ns,
        }
    }
}

/// One-time token creation result; Debug intentionally excludes its bearer token.
pub struct CreatedToken {
    /// Non-secret token view.
    pub record_view: TokenView,
    /// Bearer token, returned once to the caller and zeroized on drop.
    pub token: Zeroizing<String>,
}

impl std::fmt::Debug for CreatedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedToken")
            .field("record_view", &self.record_view)
            .field("token", &"<REDACTED>")
            .finish()
    }
}

/// Token issuance failure.
#[derive(Debug, thiserror::Error)]
pub enum TokenCreateError {
    /// Invalid requested token lifetime.
    #[error("token lifetime is invalid")]
    TokenTtlInvalid,
    /// Owner already has an active token with this label.
    #[error("token label already exists")]
    LabelExists,
    /// Owner reached the active-token cap.
    #[error("active token limit reached")]
    LimitReached,
    /// Label or database allowlist violates shape constraints.
    #[error("token fields are invalid")]
    InvalidInput,
    /// Mutation is unavailable on this node.
    #[error("token mutation is unavailable on a read-only node")]
    ReadOnlyNode,
    /// Token does not exist in durable storage.
    #[error("token not found")]
    NotFound,
    /// Durable store failed; no memory publication was made.
    #[error("token store failed")]
    Store(#[source] TokenStoreError),
    /// Repeated generated token identifiers collided with durable records.
    #[error("token identifier collision retries exhausted")]
    IdCollision,
    /// Entropy source failed.
    #[error("token entropy unavailable")]
    Entropy,
    /// Current clock could not be read.
    #[error("token clock unavailable")]
    Clock,
}

/// Cheap process-local service counters.
#[derive(Debug, Default)]
pub struct TokenCounters {
    created: std::sync::atomic::AtomicU64,
    revoked: std::sync::atomic::AtomicU64,
    verified_ok: std::sync::atomic::AtomicU64,
    verified_failed: std::sync::atomic::AtomicU64,
}

impl TokenCounters {
    /// Number of token creations accepted.
    pub fn created(&self) -> u64 {
        self.created.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of first-time token revocations accepted.
    pub fn revoked(&self) -> u64 {
        self.revoked.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of successful verifications.
    pub fn verified_ok(&self) -> u64 {
        self.verified_ok.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of failed verifications.
    pub fn verified_failed(&self) -> u64 {
        self.verified_failed
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn count_created(&self) {
        self.created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn count_revoked(&self) {
        self.revoked
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn count_verified(&self, success: bool) {
        let counter = if success {
            &self.verified_ok
        } else {
            &self.verified_failed
        };
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
