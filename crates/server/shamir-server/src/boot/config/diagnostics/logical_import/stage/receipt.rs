//! Import receipt written into the staged (then published) data directory.
//!
//! Reserved path: `<data-dir>/.logical-import-receipt.json`. This name does
//! not collide with any engine or server file (engine stores live in
//! `shamir_db_meta.redb` / `shamir_db_*.redb`, the user directory in
//! `users/`, server metadata in `server_meta/`, the stamp in
//! `format.stamp`). A complete existing target is accepted as an idempotent
//! retry ONLY when this receipt matches the artifact digest and the
//! effective options AND an integrity check passes.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::super::error::LogicalImportError;

pub(crate) const RECEIPT_NAME: &str = ".logical-import-receipt.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Receipt {
    /// Lowercase hex SHA-256 of the whole artifact file.
    pub artifact_sha256: String,
    /// Source database name inside the artifact.
    pub source_db: String,
    /// Effective imported database name (after `--as-db`).
    pub imported_db: String,
    pub schema_only: bool,
    /// Format epoch stamped into the data directory.
    pub format_epoch: u64,
    /// Generator identification for diagnostics.
    pub generator: String,
}

impl Receipt {
    pub(crate) fn write(&self, dir: &Path) -> Result<(), LogicalImportError> {
        let path = dir.join(RECEIPT_NAME);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| receipt_err(format!("cannot encode receipt: {e}")))?;
        std::fs::write(&path, json)
            .map_err(|e| receipt_err(format!("cannot write {}: {e}", path.display())))
    }

    /// `None` when the target has no receipt (never imported, or a foreign
    /// non-empty directory). `Some` only for a parseable receipt.
    pub(crate) fn read(dir: &Path) -> Result<Option<Self>, LogicalImportError> {
        let path = dir.join(RECEIPT_NAME);
        let json = match std::fs::read_to_string(&path) {
            Ok(json) => json,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(receipt_err(format!("cannot read {}: {e}", path.display()))),
        };
        serde_json::from_str(&json)
            .map(Some)
            .map_err(|e| receipt_err(format!("{} is corrupt: {e}", path.display())))
    }

    /// Idempotent-retry comparison: digest plus every effective option.
    pub(crate) fn matches(
        &self,
        artifact_sha256: &str,
        source_db: &str,
        imported_db: &str,
        schema_only: bool,
    ) -> bool {
        self.artifact_sha256 == artifact_sha256
            && self.source_db == source_db
            && self.imported_db == imported_db
            && self.schema_only == schema_only
    }
}

fn receipt_err(message: String) -> LogicalImportError {
    LogicalImportError::Receipt(message)
}
