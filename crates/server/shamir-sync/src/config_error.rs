/// Configuration error covering both load failure and validation failure.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Ktav parser rejected the input. Wraps the formatted ktav error.
    #[error("config parse: {0}")]
    Parse(String),
    /// One of the [`Config::validate`] invariants was violated.
    #[error("config validation: {0}")]
    Validation(String),
    /// Underlying I/O error (file missing, permission denied, etc.).
    #[error("config io: {0}")]
    Io(#[from] std::io::Error),
}
