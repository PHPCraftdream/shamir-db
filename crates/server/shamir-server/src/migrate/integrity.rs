use std::path::Path;

use crate::tables_registry::TablesRegistry;
use shamir_sync::format_stamp::migration::MigrationError;

/// Check only the offline wire-tables registry decoder; this does not verify database health.
pub(crate) fn check_wire_tables_registry(dir: &Path) -> Result<(), MigrationError> {
    TablesRegistry::open(dir)
        .map(drop)
        .map_err(|error| MigrationError::Io {
            dir: dir.to_path_buf(),
            source: std::io::Error::other(format!("wire_tables registry decode failed: {error}")),
        })
}
