use shamir_query_types::admin::{
    AccessTreeOp, AddGroupMemberOp, ChgrpOp, ChmodOp, ChownOp, CreateGroupOp, DropGroupOp,
    GroupRef, RemoveGroupMemberOp, RenameGroupOp, ResourceRef,
};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

// ============================================================================
// Access-tree introspection
// ============================================================================

/// Request the access-control tree. Returns a builder for optional depth/db.
pub fn access_tree() -> AccessTree {
    AccessTree {
        depth: None,
        db: None,
    }
}

/// Builder for [`AccessTreeOp`].
pub struct AccessTree {
    depth: Option<u32>,
    db: Option<String>,
}

impl AccessTree {
    /// Cap the resource hierarchy depth.
    pub fn depth(mut self, depth: u32) -> Self {
        self.depth = Some(depth);
        self
    }

    /// Restrict the tree to a single database.
    pub fn db(mut self, db: impl Into<String>) -> Self {
        self.db = Some(db.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::AccessTree(AccessTreeOp {
            access_tree: true,
            depth: self.depth,
            db: self.db,
        })
    }
}

impl From<AccessTree> for BatchOp {
    fn from(b: AccessTree) -> Self {
        b.build()
    }
}

impl IntoBatchOp for AccessTree {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

// ============================================================================
// Access-control DDL (chmod / chown / chgrp)
// ============================================================================

/// Change mode bits on a resource. Returns a builder (HMAC-gated, see
/// [`Chmod::hmac`]).
pub fn chmod(resource: ResourceRef, mode: u16) -> Chmod {
    Chmod {
        resource,
        mode,
        hmac: None,
    }
}

/// Builder for [`ChmodOp`].
pub struct Chmod {
    resource: ResourceRef,
    mode: u16,
    hmac: Option<String>,
}

impl Chmod {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_chmod(resource, mode)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::Chmod(ChmodOp {
            chmod: self.resource,
            mode: self.mode,
            hmac: self.hmac,
        })
    }
}

impl From<Chmod> for BatchOp {
    fn from(b: Chmod) -> Self {
        b.build()
    }
}

impl IntoBatchOp for Chmod {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Change owner on a resource. Returns a builder (HMAC-gated, see
/// [`Chown::hmac`]).
pub fn chown(resource: ResourceRef, owner: u64) -> Chown {
    Chown {
        resource,
        owner,
        hmac: None,
    }
}

/// Builder for [`ChownOp`].
pub struct Chown {
    resource: ResourceRef,
    owner: u64,
    hmac: Option<String>,
}

impl Chown {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_chown(resource, owner)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::Chown(ChownOp {
            chown: self.resource,
            owner: self.owner,
            hmac: self.hmac,
        })
    }
}

impl From<Chown> for BatchOp {
    fn from(b: Chown) -> Self {
        b.build()
    }
}

impl IntoBatchOp for Chown {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Change group on a resource. Pass `None` to clear the group. Returns a
/// builder (HMAC-gated, see [`Chgrp::hmac`]).
pub fn chgrp(resource: ResourceRef, group: Option<u64>) -> ChgrpBuilder {
    ChgrpBuilder {
        resource,
        group,
        hmac: None,
    }
}

/// Builder for [`ChgrpOp`].
pub struct ChgrpBuilder {
    resource: ResourceRef,
    group: Option<u64>,
    hmac: Option<String>,
}

impl ChgrpBuilder {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_chgrp(resource, group)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::Chgrp(ChgrpOp {
            chgrp: self.resource,
            group: self.group,
            hmac: self.hmac,
        })
    }
}

impl From<ChgrpBuilder> for BatchOp {
    fn from(b: ChgrpBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for ChgrpBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

// ============================================================================
// Group DDL
// ============================================================================

/// Create a new group.
pub fn create_group(name: impl Into<String>) -> BatchOp {
    BatchOp::CreateGroup(CreateGroupOp {
        create_group: name.into(),
    })
}

/// Drop a group by reference (name or id). Returns a builder for optional flags.
pub fn drop_group(group: GroupRef) -> DropGroup {
    DropGroup {
        group,
        if_exists: false,
    }
}

/// Builder for [`DropGroupOp`].
pub struct DropGroup {
    group: GroupRef,
    if_exists: bool,
}

impl DropGroup {
    /// Enable `IF EXISTS` semantics: dropping a non-existent group is
    /// a silent no-op (`existed: false`) instead of an error.
    pub fn if_exists(mut self) -> Self {
        self.if_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropGroup(DropGroupOp {
            drop_group: self.group,
            if_exists: self.if_exists,
        })
    }
}

impl From<DropGroup> for BatchOp {
    fn from(b: DropGroup) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropGroup {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Rename a group by reference (name or id) to `to`. Because groups are
/// id-keyed, this only updates the display name; members and resource
/// references (which store the group id) are unaffected.
pub fn rename_group(group: GroupRef, to: impl Into<String>) -> BatchOp {
    BatchOp::RenameGroup(RenameGroupOp {
        rename_group: group,
        to: to.into(),
    })
}

/// Add a user to a group.
pub fn add_group_member(group: GroupRef, user: u64) -> BatchOp {
    BatchOp::AddGroupMember(AddGroupMemberOp {
        add_group_member: group,
        user,
    })
}

/// Remove a user from a group.
pub fn remove_group_member(group: GroupRef, user: u64) -> BatchOp {
    BatchOp::RemoveGroupMember(RemoveGroupMemberOp {
        remove_group_member: group,
        user,
    })
}
