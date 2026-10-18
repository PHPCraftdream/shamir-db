use shamir_query_types::auth::{CreateUserOp, DropUserOp, GrantRoleOp, RevokeRoleOp, SecretString};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Create a user with a plaintext password. Returns a builder for optional
/// roles/database-scope. HMAC-gated (see [`CreateUser::hmac`]).
pub fn create_user(name: impl Into<String>, password: impl Into<String>) -> CreateUser {
    CreateUser {
        name: name.into(),
        password: password.into(),
        roles: Vec::new(),
        database: None,
        hmac: None,
    }
}

/// Builder for [`CreateUserOp`].
pub struct CreateUser {
    name: String,
    password: String,
    roles: Vec<String>,
    database: Option<String>,
    hmac: Option<String>,
}

impl CreateUser {
    /// Assign roles to the new user.
    pub fn roles(mut self, roles: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.roles = roles.into_iter().map(Into::into).collect();
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
