//! Access-control DDL operation DTOs (chmod / chown / chgrp, group CRUD).

use serde::{Deserialize, Serialize};

use shamir_types::access::ResourcePath;

// ============================================================================
// ResourceRef — wire-friendly securable resource reference
// ============================================================================

/// A JSON-friendly reference to a securable resource that maps to
/// [`ResourcePath`].
///
/// # Shapes
///
/// ```json
/// { "database": "mydb" }
/// { "store": ["mydb", "main"] }
/// { "table": ["mydb", "main", "users"] }
/// { "function": "my_fn" }
/// { "function_namespace": true }
/// ```
///
/// Each variant is a single-key object so the discriminator is unambiguous.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceRef {
    /// A database by name.
    Database { database: String },
    /// A store (repo) by `[db, store]`.
    Store { store: [String; 2] },
    /// A table by `[db, store, table]`.
    Table { table: [String; 3] },
    /// A function by name.
    Function { function: String },
    /// The function namespace singleton.
    FunctionNamespace { function_namespace: bool },
}

impl ResourceRef {
    /// Convert into the engine-level [`ResourcePath`].
    pub fn to_path(&self) -> Option<ResourcePath> {
        match self {
            ResourceRef::Database { database } => Some(ResourcePath::database(database)),
            ResourceRef::Store { store: [db, s] } => Some(ResourcePath::store(db, s)),
            ResourceRef::Table { table: [db, s, t] } => Some(ResourcePath::table(db, s, t)),
            ResourceRef::Function { function } => Some(ResourcePath::function(function)),
            ResourceRef::FunctionNamespace { .. } => Some(ResourcePath::FunctionNamespace),
        }
    }
}

// ============================================================================
// Group reference — name or numeric id
// ============================================================================

/// Reference to a group — either by name or by numeric id.
///
/// ```json
/// { "name": "devs" }
/// { "id": 3 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GroupRef {
    Name { name: String },
    Id { id: u64 },
}

// ============================================================================
// Access DDL operations
// ============================================================================

/// Change mode bits on a securable resource.
///
/// ```json
/// { "chmod": { "table": ["db", "main", "users"] }, "mode": 448 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChmodOp {
    pub chmod: ResourceRef,
    pub mode: u16,
}

/// Change owner on a securable resource.
///
/// ```json
/// { "chown": { "table": ["db", "main", "users"] }, "owner": 7 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChownOp {
    pub chown: ResourceRef,
    pub owner: u64,
}

/// Change group on a securable resource. `group: null` clears the group.
///
/// ```json
/// { "chgrp": { "table": ["db", "main", "users"] }, "group": 3 }
/// { "chgrp": { "table": ["db", "main", "users"] }, "group": null }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChgrpOp {
    pub chgrp: ResourceRef,
    pub group: Option<u64>,
}

/// Create a new group.
///
/// ```json
/// { "create_group": "devs" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateGroupOp {
    pub create_group: String,
}

/// Drop an existing group by name or id.
///
/// ```json
/// { "drop_group": { "name": "devs" } }
/// { "drop_group": { "id": 3 } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropGroupOp {
    pub drop_group: GroupRef,
}

/// Add a user to a group.
///
/// ```json
/// { "add_group_member": { "name": "devs" }, "user": 42 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddGroupMemberOp {
    pub add_group_member: GroupRef,
    pub user: u64,
}

/// Remove a user from a group.
///
/// ```json
/// { "remove_group_member": { "name": "devs" }, "user": 42 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoveGroupMemberOp {
    pub remove_group_member: GroupRef,
    pub user: u64,
}
