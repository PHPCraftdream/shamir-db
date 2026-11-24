//! Online backup server adapter (R2a step 3a): the `OnlineBackupPort`
//! implementation over the engine facade, the per-server session registry,
//! the background runner and the admin-dispatch bridge (design D6 — the
//! `StartBackup`/`BackupStatus`/`CancelBackup` admin ops reach the registry
//! through the engine's injected `OnlineBackupAdminPort`).

pub mod adapter;
pub mod metrics;
mod progress_attribution;
pub mod registry;
pub(crate) mod schedule;

mod admin_port;

pub use adapter::{stage_hint, EngineBackupPort, PortRunState};
pub use registry::{BackupStatusSnapshot, OnlineBackupRegistry, StartBackupError};
