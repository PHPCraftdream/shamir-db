//! Admin integration tests.

#[path = "activity/mod.rs"]
mod activity;
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
#[path = "logical_dump/mod.rs"]
mod logical_dump;
#[path = "logical_import/mod.rs"]
mod logical_import;
#[path = "ops/migration_api_gating.rs"]
mod migration_api_gating;
#[path = "identity/permission_e2e/mod.rs"]
mod permission_e2e;
#[path = "shell/mod.rs"]
mod shell_tests;
#[path = "identity/user_admin_port_wire.rs"]
mod user_admin_port_wire;

/// The REAL production shell package, included in-tree so the scripted
/// terminal-restore tests (`shell_tests::output`) drive the actual
/// orchestration (`read_secret_with_writer` Prompt path) through the
/// [`Terminal`] seam — the production module reaches `crate::shell` paths
/// against THIS crate root, no duplicate module exists. `shell::run` is
/// additionally invoked (`--help`) so the included production tree is a
/// live root for dead-code analysis.
#[path = "../../shell/mod.rs"]
mod shell;
#[path = "identity/user_directory/mod.rs"]
mod user_directory;
