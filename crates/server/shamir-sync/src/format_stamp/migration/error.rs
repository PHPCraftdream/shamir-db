use std::path::PathBuf;

use thiserror::Error;

use crate::format_stamp::model::StampError;

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("format migration refused by stamp check: {source}")]
    Refused {
        #[source]
        source: StampError,
    },
    #[error(
        "broken format migration chain from epoch {from}: expected {expected}, found {found:?}"
    )]
    BrokenChain {
        from: u64,
        expected: u64,
        found: Option<u64>,
    },
    #[error("format migration journal in {dir} failed")]
    Journal {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("format migration step {module} failed: {source}")]
    StepFailed {
        module: &'static str,
        #[source]
        source: Box<MigrationError>,
    },
    #[error("format migration backup gate failed: {message}")]
    BackupGate { message: String },
    #[error("the data directory is already in use by a running server or maintenance command")]
    ServerRunning,
    #[error("format migration I/O in {dir} failed")]
    Io {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
