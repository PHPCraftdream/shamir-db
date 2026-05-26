//! Auth-related DTOs (User, Role, Permission). Permission-check logic
//! (`SessionPermissions::check_batch`) lives in shamir-engine.

pub mod types;

pub use types::{
    Action, CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, Effect, GrantRoleOp, Permission,
    Resource, RevokeRoleOp, Role, User,
};
