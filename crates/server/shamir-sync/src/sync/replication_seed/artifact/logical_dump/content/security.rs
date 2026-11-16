use serde::{Deserialize, Serialize};

/// Credential-free identity projection; imported accounts are disabled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityItem {
    pub logical_name: String,
    pub kind: IdentityKind,
    pub disabled_on_import: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityKind {
    User,
    Group,
}

/// ACL metadata uses stable logical principal names rather than internal IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AclItem {
    pub resource: ResourceRef,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub mode: u16,
    pub grants: Vec<AclGrant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRef {
    pub database: String,
    pub repository: Option<String>,
    pub table: Option<String>,
    pub object: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AclGrant {
    pub principal: String,
    pub principal_kind: IdentityKind,
    pub actions: Vec<AclAction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AclAction {
    Read,
    Write,
    Create,
    Delete,
    Execute,
    List,
    Manage,
}
