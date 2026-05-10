//! Auth types — Resource, Action, Permission, Role, User, and auth operations.

use serde::{Deserialize, Serialize};

use crate::db::query::filter::Filter;

// ============================================================================
// Core auth types
// ============================================================================

/// Resource scope — what the permission applies to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "lowercase")]
pub enum Resource {
    Global,
    Database {
        database: String,
    },
    Repo {
        database: String,
        repo: String,
    },
    Table {
        database: String,
        repo: String,
        table: String,
    },
}

impl Resource {
    /// Specificity level: global=0, database=1, repo=2, table=3.
    pub fn specificity(&self) -> u8 {
        match self {
            Resource::Global => 0,
            Resource::Database { .. } => 1,
            Resource::Repo { .. } => 2,
            Resource::Table { .. } => 3,
        }
    }

    /// Check if this resource covers the target resource.
    pub fn covers(&self, target: &Resource) -> bool {
        match (self, target) {
            (Resource::Global, _) => true,
            (Resource::Database { database: d1 }, Resource::Database { database: d2 }) => d1 == d2,
            (Resource::Database { database: d1 }, Resource::Repo { database: d2, .. }) => d1 == d2,
            (Resource::Database { database: d1 }, Resource::Table { database: d2, .. }) => d1 == d2,
            (Resource::Repo { database: d1, repo: r1 }, Resource::Repo { database: d2, repo: r2 }) => d1 == d2 && r1 == r2,
            (Resource::Repo { database: d1, repo: r1 }, Resource::Table { database: d2, repo: r2, .. }) => d1 == d2 && r1 == r2,
            (Resource::Table { database: d1, repo: r1, table: t1 }, Resource::Table { database: d2, repo: r2, table: t2 }) => d1 == d2 && r1 == r2 && t1 == t2,
            _ => false,
        }
    }
}

/// Action type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Read,
    Insert,
    Update,
    Delete,
    Create,
    Drop,
    ManageUsers,
    ManageRoles,
    All,
}

impl Action {
    /// Check if this action matches the requested action.
    pub fn matches(&self, requested: Action) -> bool {
        *self == Action::All || *self == requested
    }
}

/// Permission effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Deny,
}

/// Single permission entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Permission {
    pub effect: Effect,
    pub actions: Vec<Action>,
    pub resource: Resource,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "where")]
    pub row_filter: Option<Filter>,
}

/// Role — named set of permissions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub permissions: Vec<Permission>,
}

/// User document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    pub password_hash: String,
    pub roles: Vec<String>,
    /// Arbitrary user profile fields (for $user references in row filters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<serde_json::Value>,
}

// ============================================================================
// Auth operations (for BatchOp)
// ============================================================================

/// Create a user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateUserOp {
    pub create_user: String,
    pub password: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<serde_json::Value>,
}

/// Drop a user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropUserOp {
    pub drop_user: String,
}

/// Create a role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateRoleOp {
    pub create_role: String,
    pub permissions: Vec<Permission>,
}

/// Drop a role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropRoleOp {
    pub drop_role: String,
}

/// Grant a role to a user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrantRoleOp {
    pub grant_role: String,
    pub user: String,
}

/// Revoke a role from a user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevokeRoleOp {
    pub revoke_role: String,
    pub user: String,
}
