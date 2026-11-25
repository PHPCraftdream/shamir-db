//! Boot-time construction of the online logical-export job service
//! (R3-2b). Kept out of `launch.rs` to hold it under the layout line
//! limit; the launcher passes the config, the shared gate (present iff a
//! root is configured), the engine handles, the node mode, and the session store.

use std::path::Path;
use std::sync::Arc;

use shamir_connect::server::SessionStore;
use shamir_db::engine::db_instance::logical_export::MaintenanceGate;
use shamir_db::ShamirDb;
use tokio_util::sync::CancellationToken;

use crate::boot::config::diagnostics::logical::export_online::{
    ExportRegistry, ExportRoot, ExportRuntime, OnlineExport,
};
use crate::config::Config;
use crate::db_handler::{NodeMode, ShamirDbHandler};
use crate::server::boot_error::BootError;
use crate::user_directory::FjallUserDirectory;

/// Build the online-export service when a root AND the shared gate are
/// configured (`None` root → `None` service: the wire ops refuse with the
/// stable `online_export_not_configured` code). Enable the runner only on
/// ReadWrite nodes without experimental migration. Otherwise, a start
/// against the installed service returns `online_export_unavailable`.
///
/// Root open follows the validated-root rules (private directory,
/// exclusive lock, stale-temp cleanup). The source storage epoch derives
/// from the format stamp with the SAME arms the offline export driver
/// uses; an unreadable stamp fails boot.
pub(super) fn maybe_online_export(
    config: &Config,
    gate: Option<&MaintenanceGate>,
    db: Arc<ShamirDb>,
    directory: Arc<FjallUserDirectory>,
    shutdown: CancellationToken,
    node_mode: NodeMode,
    sessions: Arc<SessionStore>,
) -> Result<Option<Arc<OnlineExport>>, BootError> {
    let (Some(root_path), Some(gate)) = (config.security.export_root.as_deref(), gate) else {
        return Ok(None);
    };
    let epoch = source_epoch(&config.data_dir)?;
    let service = OnlineExport::new(
        Arc::new(ExportRegistry::default()),
        Arc::new(ExportRoot::open(root_path, &config.data_dir)?),
        gate.clone(),
        db,
        directory,
        epoch,
        ExportRuntime {
            runtime: tokio::runtime::Handle::current(),
            shutdown,
            sessions: Some(sessions),
        },
    );
    let enabled =
        node_mode == NodeMode::ReadWrite && !config.security.enable_experimental_migration_api;
    let service = if enabled { service.enable() } else { service };
    Ok(Some(Arc::new(service)))
}

/// Install the service on the handler builder (`None` service → builder
/// unchanged, so the handler keeps its fail-closed `online_export: None`).
pub(super) fn wire_handler(
    builder: ShamirDbHandler,
    service: Option<&Arc<OnlineExport>>,
) -> ShamirDbHandler {
    match service {
        Some(svc) => builder.with_online_export(Arc::clone(svc)),
        None => builder,
    }
}

/// The source storage epoch, with the SAME arms the offline export driver
/// uses: a valid, non-adopted stamp carries the epoch; a valid adopted
/// stamp or an absent stamp carries none; anything else fails boot.
fn source_epoch(data_dir: &Path) -> Result<Option<u32>, BootError> {
    match shamir_sync::format_stamp::inspect(data_dir) {
        shamir_sync::format_stamp::StampState::Valid(stamp)
            if stamp.created_by().adopted != Some(true) =>
        {
            Ok(Some(u32::try_from(stamp.format_epoch()).map_err(|e| {
                BootError::ShamirDbInit(format!("export source epoch: {e}"))
            })?))
        }
        shamir_sync::format_stamp::StampState::Valid(_)
        | shamir_sync::format_stamp::StampState::Absent { .. } => Ok(None),
        state => Err(BootError::ShamirDbInit(format!(
            "export source stamp unreadable: {state:?}"
        ))),
    }
}
