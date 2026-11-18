//! File-only compatibility check for the staged wire-tables registry.
//!
//! The online restore swap publishes a staged `data_dir` that the server
//! will boot from. Boot (`shamir-server` `TablesRegistry::open`, replayed
//! from `server_launcher` `launch.rs`) reads `<data_dir>/wire_tables.mpack`
//! and decodes it into a `RegistrySnapshot` — a MessagePack map from
//! `"db.repo"` keys to arrays of table-name strings (a single
//! `#[serde(flatten)]`ed string-map). A staged file whose bytes hash
//! correctly but whose shape the decoder rejects is FATAL at boot, after
//! the swap has already replaced the live directory. This module performs
//! the same decode on the STAGED copy before the swap so such a file
//! refuses typed instead.
//!
//! Scope: shape compatibility with the boot decoder only — NOT validation
//! of any independent server subsystem (repos, tables, stores). The shape
//! mirror below is deliberately private and structurally identical to the
//! boot `RegistrySnapshot` (`map of strings -> vectors of strings` via a
//! flattened wrapper); no server code is imported.
//!
//! Acceptance mirrors `TablesRegistry::open` exactly: a missing file and
//! an empty file are both treated as an empty registry; anything else must
//! decode. I/O failures and decode failures both refuse as
//! [`RestoreError::Consistency`] with the pseudo-repository
//! `(wire_tables)` naming the operation.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::RestoreError;

/// File name relative to `data_dir`; mirrors the boot registry's
/// `tables_registry::FILENAME`.
const WIRE_TABLES_FILENAME: &str = "wire_tables.mpack";

/// Pseudo-repository label used in the typed refusal so the error names
/// the artifact rather than any captured repository.
const PSEUDO_REPO: &str = "(wire_tables)";

/// Private shape mirror of the boot decoder's target
/// (`RegistrySnapshot`): a MessagePack map from `"db.repo"` string keys to
/// vectors of table-name strings, behind a flattened wrapper. A
/// `BTreeMap` deserializes with the same shape rules as the boot
/// `IndexMap` (order is irrelevant to decode acceptance).
#[derive(Debug, Deserialize)]
struct StagedWireTablesSnapshot {
    #[serde(flatten)]
    _tables_by_repo: BTreeMap<String, Vec<String>>,
}

/// Validate the staged copy's `wire_tables.mpack` against the boot
/// decoder's shape. Missing and empty files pass; any other content must
/// decode exactly the way `TablesRegistry::open` decodes at boot.
pub(super) async fn validate_staged_wire_tables(stage: &Path) -> Result<(), RestoreError> {
    let path = stage.join(WIRE_TABLES_FILENAME);
    let bytes = match tokio::fs::read(&path).await {
        // Boot treats an empty file as an empty registry.
        Ok(bytes) if bytes.is_empty() => return Ok(()),
        Ok(bytes) => bytes,
        // Absent registry == empty registry at boot; nothing to check.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(RestoreError::Consistency {
                repo: PSEUDO_REPO.into(),
                reason: format!(
                    "staging wire-table registry check: reading {}: {e}",
                    path.display()
                ),
            })
        }
    };
    if let Err(e) = rmp_serde::from_slice::<StagedWireTablesSnapshot>(&bytes) {
        return Err(RestoreError::Consistency {
            repo: PSEUDO_REPO.into(),
            reason: format!(
                "staged {} is not decodable by the server boot registry \
                 (map of \"db.repo\" -> arrays of table names): {e} — the restored \
                 copy would fail to boot",
                WIRE_TABLES_FILENAME
            ),
        });
    }
    Ok(())
}
