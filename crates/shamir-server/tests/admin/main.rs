//! Admin integration tests.

#[path = "identity/auth_ok_hints_e2e.rs"]
mod auth_ok_hints_e2e;
#[path = "ops/backup_restore_e2e/mod.rs"]
mod backup_restore_e2e;
#[path = "ops/bootstrap_token_lifecycle_e2e.rs"]
mod bootstrap_token_lifecycle_e2e;
#[path = "identity/change_password_e2e.rs"]
mod change_password_e2e;
#[path = "../common/mod.rs"]
mod common;
#[path = "ops/config.rs"]
mod config;
#[path = "ops/config_unknown_section.rs"]
mod config_unknown_section;
#[path = "ops/doctor_e2e.rs"]
mod doctor_e2e;
#[path = "ops/migration_api_gating.rs"]
mod migration_api_gating;
#[path = "identity/permission_e2e/mod.rs"]
mod permission_e2e;
#[path = "identity/user_admin_port_wire.rs"]
mod user_admin_port_wire;
#[path = "identity/user_directory/mod.rs"]
mod user_directory;
