//! Auth-related DTOs (User, Role, Permission). Permission-check logic
//! (`SessionPermissions::check_batch`) lives in shamir-engine.

pub mod types;

pub use shamir_types::secret::SecretString;
pub use types::{
    Action, CreateUserOp, DropUserOp, Effect, GrantRoleOp, Permission, Resource, RevokeRoleOp,
    Role, User,
};
