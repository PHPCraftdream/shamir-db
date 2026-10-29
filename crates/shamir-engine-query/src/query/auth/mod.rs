//! Auth-related operations module.
//!
//! DTOs (User / Role / CreateUserOp etc.) live in
//! `shamir-query-types::auth`. SessionPermissions + check_batch logic
//! stays here in `session.rs` because it touches batch-planning
//! internals.
//!
//! `SessionPermissions` is test-only RBAC/RLS scaffolding (see its own
//! doc comment) — the live access-control model is DAC via
//! `ShamirDb::execute_as`, NOT this role matrix. To keep it from being
//! mistaken for a real, enforced access-control model, it is gated out
//! of the crate's public API by default: reachable only from this
//! crate's own unit tests (`cfg(test)`) or with the `test-util` feature
//! explicitly enabled (needed by `benches/permission_check.rs`, a
//! separate non-test compilation unit — see that bench's own doc
//! comment). Mirrors the `test-util`-gated seam in
//! `repo/repo_instance.rs::install_table_for_test`.

#[cfg(any(test, feature = "test-util"))]
mod session;

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-util"))]
pub use session::SessionPermissions;
pub use shamir_query_types::auth::{
    Action, CreateUserOp, DropUserOp, Effect, GrantRoleOp, Permission, Resource, RevokeRoleOp,
    Role, SecretString, User,
};
