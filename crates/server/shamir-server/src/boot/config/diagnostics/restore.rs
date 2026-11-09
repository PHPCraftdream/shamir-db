//! Server-facing restore wrapper with the concrete metadata liveness probe.

pub use shamir_sync::restore::*;

use std::path::Path;

use crate::server_meta::ServerMetaStore;
use shamir_sync::meta_port::MetaError;

/// Restore `from` into `data_dir`, probing server metadata unless forced.
pub fn restore(from: &Path, data_dir: &Path, force: bool) -> Result<RestoreReport, RestoreError> {
    restore_with_probe(from, data_dir, force, probe_not_running)
}

/// Attempt to open `data_dir/server_meta` (mirrors the boot-time open) purely
/// as a liveness probe. `Ok(())` on success (store opened and was immediately
/// dropped, releasing the lock) or when the metadata directory does not exist
/// yet (a fresh target). `Err` on lock contention (server running) or any
/// other open failure (fail-closed).
fn probe_not_running(data_dir: &Path) -> Result<(), RestoreError> {
    let meta_dir = data_dir.join("server_meta");
    if !meta_dir.exists() {
        return Ok(()); // fresh target — nothing to probe
    }
    match ServerMetaStore::open_or_init(&meta_dir) {
        Ok(store) => {
            drop(store); // release the lock immediately
            Ok(())
        }
        Err(MetaError::Fjall(fjall::Error::Locked)) => {
            Err(RestoreError::ServerRunning(data_dir.to_path_buf()))
        }
        Err(other) => Err(RestoreError::ProbeFailed(data_dir.to_path_buf(), other)),
    }
}
