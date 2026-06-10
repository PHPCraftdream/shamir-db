pub mod coordinator;
pub mod shadow_key;
pub mod shadow_log;

pub use coordinator::{MigrationCoordinator, MigrationPhase, MigrationState};
pub use shadow_log::{MigrationShadowLog, ShadowEntry, ShadowOp};

#[cfg(test)]
mod tests;
