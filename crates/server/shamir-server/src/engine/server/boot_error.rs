//! Boot-time error type.

use std::path::PathBuf;

/// Errors that can happen during boot.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error("config: {0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("format stamp: {0}")]
    FormatStamp(#[from] shamir_sync::format_stamp::StampError),
    #[error("server_meta: {0}")]
    ServerMeta(String),
    #[error("user_directory: {0}")]
    UserDirectory(String),
    #[error("token service: {0}")]
    TokenService(String),
    #[error("counters: {0}")]
    Counters(String),
    #[error("audit_appender: {0}")]
    AuditAppender(String),
    #[error("audit recovery: {0}")]
    AuditRecovery(#[source] crate::audit_appender::AuditRecoveryError),
    #[error("audit recovery: failed to read the audit chain key")]
    AuditKeyRead(#[source] crate::server_meta::MetaError),
    #[error("audit recovery: failed to read the server metadata checkpoint")]
    AuditCheckpointRead(#[source] crate::server_meta::MetaError),
    #[error("audit recovery: failed to persist the appender checkpoint")]
    AuditAppenderCheckpoint(#[source] std::io::Error),
    #[error("audit recovery: failed to persist the server metadata checkpoint")]
    AuditMetaCheckpoint(#[source] crate::server_meta::MetaError),
    #[error("shamir_db init: {0}")]
    ShamirDbInit(String),
    #[error("tls: {0}")]
    Tls(#[from] crate::tls::TlsError),
    #[error("bootstrap: {0}")]
    Bootstrap(#[from] crate::bootstrap::BootstrapError),
    #[error("listener bind: {0}")]
    Bind(String),
    #[error("another shamir-server instance is already using {0} (lock held); refusing to start")]
    AlreadyRunning(PathBuf),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("tables_registry: {0}")]
    TablesRegistry(#[from] crate::tables_registry::RegistryError),
}
