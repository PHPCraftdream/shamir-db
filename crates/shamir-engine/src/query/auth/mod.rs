//! Auth-related operations module.
//!
//! DTOs (User / Role / CreateUserOp etc.) live in
//! `shamir-query-types::auth`. SessionPermissions + check_batch logic
//! stays here in `session.rs` because it touches batch-planning
//! internals.

mod session;

pub use session::SessionPermissions;
pub use shamir_query_types::auth::{
    Action, CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, Effect, GrantRoleOp, Permission,
    RenameRoleOp, Resource, RevokeRoleOp, Role, SecretString, User,
};
