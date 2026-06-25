use shamir_query_types::auth::{
    CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, GrantRoleOp, Permission, RenameRoleOp,
    RevokeRoleOp, SecretString,
};
use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::QueryValue;

use crate::batch::IntoBatchOp;

/// Create a user with a plaintext password. Returns a builder for optional
/// roles/profile.
pub fn create_user(name: impl Into<String>, password: impl Into<String>) -> CreateUser {
    CreateUser {
        name: name.into(),
        password: password.into(),
        roles: Vec::new(),
        profile: None,
        database: None,
    }
}

/// Builder for [`CreateUserOp`].
pub struct CreateUser {
    name: String,
    password: String,
    roles: Vec<String>,
    profile: Option<QueryValue>,
    database: Option<String>,
}

impl CreateUser {
    /// Assign roles to the new user.
    pub fn roles(mut self, roles: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    /// Set the user profile.
    pub fn profile(mut self, profile: QueryValue) -> Self {
        self.profile = Some(profile);
        self
    }

    /// Scope the user to a database, allowing that database's owner to
    /// manage it without global-admin rights.
    pub fn database(mut self, database: impl Into<String>) -> Self {
        self.database = Some(database.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateUser(CreateUserOp {
            create_user: self.name,
            password: SecretString::from(self.password),
            roles: self.roles,
            profile: self.profile,
            database: self.database,
        })
    }
}

impl From<CreateUser> for BatchOp {
    fn from(b: CreateUser) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateUser {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Drop a user by name.
pub fn drop_user(name: impl Into<String>) -> DropUser {
    DropUser {
        name: name.into(),
        hmac: None,
        if_exists: false,
    }
}

/// Builder for [`DropUserOp`].
pub struct DropUser {
    name: String,
    hmac: Option<String>,
    if_exists: bool,
}

impl DropUser {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Enable `IF EXISTS` semantics: dropping a non-existent user is
    /// a silent no-op (`existed: false`) instead of an error.
    pub fn if_exists(mut self) -> Self {
        self.if_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropUser(DropUserOp {
            drop_user: self.name,
            hmac: self.hmac,
            if_exists: self.if_exists,
        })
    }
}

impl From<DropUser> for BatchOp {
    fn from(b: DropUser) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropUser {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Create a role with a set of permissions.
pub fn create_role(name: impl Into<String>, permissions: Vec<Permission>) -> BatchOp {
    BatchOp::CreateRole(CreateRoleOp {
        create_role: name.into(),
        permissions,
    })
}

/// Drop a role by name.
pub fn drop_role(name: impl Into<String>) -> DropRole {
    DropRole {
        name: name.into(),
        hmac: None,
        if_exists: false,
    }
}

/// Builder for [`DropRoleOp`].
pub struct DropRole {
    name: String,
    hmac: Option<String>,
    if_exists: bool,
}

impl DropRole {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Enable `IF EXISTS` semantics: dropping a non-existent role is
    /// a silent no-op (`existed: false`) instead of an error.
    pub fn if_exists(mut self) -> Self {
        self.if_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropRole(DropRoleOp {
            drop_role: self.name,
            hmac: self.hmac,
            if_exists: self.if_exists,
        })
    }
}

impl From<DropRole> for BatchOp {
    fn from(b: DropRole) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropRole {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Grant a role to a user.
pub fn grant_role(role: impl Into<String>, user: impl Into<String>) -> BatchOp {
    BatchOp::GrantRole(GrantRoleOp {
        grant_role: role.into(),
        user: user.into(),
    })
}

/// Revoke a role from a user.
pub fn revoke_role(role: impl Into<String>, user: impl Into<String>) -> BatchOp {
    BatchOp::RevokeRole(RevokeRoleOp {
        revoke_role: role.into(),
        user: user.into(),
    })
}

/// Rename a role (`from` → `to`). Re-keys the role record and rewrites
/// `roles` references in every user that holds the old name.
pub fn rename_role(from: impl Into<String>, to: impl Into<String>) -> BatchOp {
    BatchOp::RenameRole(RenameRoleOp {
        rename_role: from.into(),
        to: to.into(),
    })
}
