//! Access-control DDL operation DTOs (chmod / chown / chgrp, group CRUD).

use serde::{Deserialize, Serialize};

use super::types::db_ops::is_false;

#[cfg(feature = "server")]
use shamir_types::access::ResourcePath;

// ============================================================================
// ResourceRef — wire-friendly securable resource reference
// ============================================================================

/// Wire-encodable reference to a securable resource that maps to
/// [`ResourcePath`].
///
/// # Shapes
///
/// ```text
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
    /// A function folder by path segments.
    FunctionFolder { function_folder: Vec<String> },
    /// The function namespace singleton.
    FunctionNamespace { function_namespace: bool },
}

#[cfg(feature = "server")]
impl ResourceRef {
    /// Convert into the engine-level [`ResourcePath`].
    pub fn to_path(&self) -> Option<ResourcePath> {
        match self {
            ResourceRef::Database { database } => Some(ResourcePath::database(database)),
            ResourceRef::Store { store: [db, s] } => Some(ResourcePath::store(db, s)),
            ResourceRef::Table { table: [db, s, t] } => Some(ResourcePath::table(db, s, t)),
            ResourceRef::Function { function } => Some(ResourcePath::function(function)),
            ResourceRef::FunctionFolder { function_folder } => {
                Some(ResourcePath::function_folder(function_folder.clone()))
            }
            ResourceRef::FunctionNamespace { .. } => Some(ResourcePath::FunctionNamespace),
        }
    }
}

// ============================================================================
// Group reference — name or numeric id
// ============================================================================

/// Reference to a group — either by name or by numeric id.
///
/// ```text
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
/// ```text
/// { "chmod": { "table": ["db", "main", "users"] }, "mode": 448 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChmodOp {
    pub chmod: ResourceRef,
    pub mode: u16,
}

/// Change owner on a securable resource.
///
/// ```text
/// { "chown": { "table": ["db", "main", "users"] }, "owner": 7 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChownOp {
    pub chown: ResourceRef,
    pub owner: u64,
}

/// Change group on a securable resource. `group: null` clears the group.
///
/// ```text
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
/// ```text
/// { "create_group": "devs" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateGroupOp {
    pub create_group: String,
}

/// Drop an existing group by name or id.
///
/// ```text
/// { "drop_group": { "name": "devs" } }
/// { "drop_group": { "id": 3 } }
/// { "drop_group": { "name": "devs" }, "if_exists": true }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropGroupOp {
    pub drop_group: GroupRef,
    /// When `true`, dropping a non-existent group is a silent no-op
    /// returning `{"existed": false}` instead of an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_exists: bool,
}

/// Rename an existing group. The group is id-keyed, so renaming only
/// updates the display name stored under the (unchanged) `group_id`;
/// members and resource references (which store `group_id`, not the name)
/// are untouched.
///
/// ```text
/// { "rename_group": { "name": "devs" }, "to": "engineers" }
/// { "rename_group": { "id": 3 }, "to": "engineers" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameGroupOp {
    pub rename_group: GroupRef,
    pub to: String,
}

/// Add a user to a group.
///
/// ```text
/// { "add_group_member": { "name": "devs" }, "user": 42 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddGroupMemberOp {
    pub add_group_member: GroupRef,
    pub user: u64,
}

/// Remove a user from a group.
///
/// ```text
/// { "remove_group_member": { "name": "devs" }, "user": 42 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoveGroupMemberOp {
    pub remove_group_member: GroupRef,
    pub user: u64,
}

// ============================================================================
// Access tree (read-only introspection)
// ============================================================================

/// Request the access-control tree: the resource hierarchy
/// (Root→Database→Store→Table) with `owner:group` and POSIX mode on
/// every node, plus the principals (users and groups with membership)
/// and the stored functions with their mode/setuid.
///
/// ```text
/// { "access_tree": true }
/// { "access_tree": true, "depth": 2 }
/// { "access_tree": true, "db": "mydb" }
/// ```
///
/// `depth` caps the resource hierarchy: `0` = root only, `1` = databases,
/// `2` = stores, `3` = tables (the default / current maximum). Functions
/// and principals are always included regardless of `depth`.
///
/// Reading the tree requires `Manage` on the root (admin authority);
/// non-admin callers are denied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessTreeOp {
    /// Discriminator flag — always `true`.
    pub access_tree: bool,
    /// Resource-depth cap (see struct docs). `None` → full depth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// Restrict the resource tree to a single database by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<String>,
}
