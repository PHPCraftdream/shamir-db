use shamir_query_types::auth::{
    CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, GrantRoleOp, Permission, RenameRoleOp,
    RevokeRoleOp, SecretString,
};
use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::QueryValue;

use crate::batch::IntoBatchOp;

/// Create a user with a plaintext password. Returns a builder for optional
/// roles/profile. HMAC-gated (see [`CreateUser::hmac`]).
pub fn create_user(name: impl Into<String>, password: impl Into<String>) -> CreateUser {
    CreateUser {
        name: name.into(),
        password: password.into(),
        roles: Vec::new(),
        profile: None,
        database: None,
        hmac: None,
    }
}

/// Builder for [`CreateUserOp`].
pub struct CreateUser {
    name: String,
    password: String,
    roles: Vec<String>,
    profile: Option<QueryValue>,
    database: Option<String>,
    hmac: Option<String>,
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

    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_create_user(username)` (never the password).
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
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
            hmac: self.hmac,
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

/// Create a role with a set of permissions. Returns a builder (HMAC-gated,
/// see [`CreateRole::hmac`]).
pub fn create_role(name: impl Into<String>, permissions: Vec<Permission>) -> CreateRole {
    CreateRole {
        name: name.into(),
        permissions,
        hmac: None,
    }
}

/// Builder for [`CreateRoleOp`].
pub struct CreateRole {
    name: String,
    permissions: Vec<Permission>,
    hmac: Option<String>,
}

impl CreateRole {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_create_role(role)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateRole(CreateRoleOp {
            create_role: self.name,
            permissions: self.permissions,
            hmac: self.hmac,
        })
    }
}

impl From<CreateRole> for BatchOp {
    fn from(b: CreateRole) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateRole {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
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

/// Grant a role to a user. Returns a builder (HMAC-gated, see
/// [`GrantRole::hmac`]) — the single most dangerous op in the system
/// (e.g. granting `superuser` to an attacker-controlled account).
pub fn grant_role(role: impl Into<String>, user: impl Into<String>) -> GrantRole {
    GrantRole {
        role: role.into(),
        user: user.into(),
        hmac: None,
    }
}

/// Builder for [`GrantRoleOp`].
pub struct GrantRole {
    role: String,
    user: String,
    hmac: Option<String>,
}

impl GrantRole {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_grant_role(role, user)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::GrantRole(GrantRoleOp {
            grant_role: self.role,
            user: self.user,
            hmac: self.hmac,
        })
    }
}

impl From<GrantRole> for BatchOp {
    fn from(b: GrantRole) -> Self {
        b.build()
    }
}

impl IntoBatchOp for GrantRole {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Revoke a role from a user. Returns a builder (HMAC-gated, see
/// [`RevokeRole::hmac`]).
pub fn revoke_role(role: impl Into<String>, user: impl Into<String>) -> RevokeRole {
    RevokeRole {
        role: role.into(),
        user: user.into(),
        hmac: None,
    }
}

/// Builder for [`RevokeRoleOp`].
pub struct RevokeRole {
    role: String,
    user: String,
    hmac: Option<String>,
}

impl RevokeRole {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_revoke_role(role, user)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RevokeRole(RevokeRoleOp {
            revoke_role: self.role,
            user: self.user,
            hmac: self.hmac,
        })
    }
}

impl From<RevokeRole> for BatchOp {
    fn from(b: RevokeRole) -> Self {
        b.build()
    }
}

impl IntoBatchOp for RevokeRole {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Rename a role (`from` → `to`). Re-keys the role record and rewrites
/// `roles` references in every user that holds the old name.
pub fn rename_role(from: impl Into<String>, to: impl Into<String>) -> BatchOp {
    BatchOp::RenameRole(RenameRoleOp {
        rename_role: from.into(),
        to: to.into(),
    })
}
