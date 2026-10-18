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

/// Create a new group. Returns a builder (HMAC-gated, see
/// [`CreateGroup::hmac`]).
pub fn create_group(name: impl Into<String>) -> CreateGroup {
    CreateGroup {
        name: name.into(),
        hmac: None,
    }
}

/// Builder for [`CreateGroupOp`].
pub struct CreateGroup {
    name: String,
    hmac: Option<String>,
}

impl CreateGroup {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_create_group(name)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateGroup(CreateGroupOp {
            create_group: self.name,
            hmac: self.hmac,
        })
    }
}

impl From<CreateGroup> for BatchOp {
    fn from(b: CreateGroup) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateGroup {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Drop a group by reference (name or id). Returns a builder for optional
/// flags (HMAC-gated, see [`DropGroup::hmac`]).
pub fn drop_group(group: GroupRef) -> DropGroup {
    DropGroup {
        group,
        if_exists: false,
        hmac: None,
    }
}

/// Builder for [`DropGroupOp`].
pub struct DropGroup {
    group: GroupRef,
    if_exists: bool,
    hmac: Option<String>,
}

impl DropGroup {
    /// Enable `IF EXISTS` semantics: dropping a non-existent group is
    /// a silent no-op (`existed: false`) instead of an error.
    pub fn if_exists(mut self) -> Self {
        self.if_exists = true;
        self
    }

    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_drop_group(group)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropGroup(DropGroupOp {
            drop_group: self.group,
            if_exists: self.if_exists,
            hmac: self.hmac,
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
/// references (which store the group id) are unaffected. Returns a
/// builder (HMAC-gated, see [`RenameGroup::hmac`]).
pub fn rename_group(group: GroupRef, to: impl Into<String>) -> RenameGroup {
    RenameGroup {
        group,
        to: to.into(),
        hmac: None,
    }
}

/// Builder for [`RenameGroupOp`].
pub struct RenameGroup {
    group: GroupRef,
    to: String,
    hmac: Option<String>,
}

impl RenameGroup {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_rename_group(group, to)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RenameGroup(RenameGroupOp {
            rename_group: self.group,
            to: self.to,
            hmac: self.hmac,
        })
    }
}

impl From<RenameGroup> for BatchOp {
    fn from(b: RenameGroup) -> Self {
        b.build()
    }
}

impl IntoBatchOp for RenameGroup {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Add a user to a group. Returns a builder (HMAC-gated, see
/// [`AddGroupMember::hmac`]).
pub fn add_group_member(group: GroupRef, user: u64) -> AddGroupMember {
    AddGroupMember {
        group,
        user,
        hmac: None,
    }
}

/// Builder for [`AddGroupMemberOp`].
pub struct AddGroupMember {
    group: GroupRef,
    user: u64,
    hmac: Option<String>,
}

impl AddGroupMember {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_add_group_member(group, user)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::AddGroupMember(AddGroupMemberOp {
            add_group_member: self.group,
            user: self.user,
            hmac: self.hmac,
        })
    }
}

impl From<AddGroupMember> for BatchOp {
    fn from(b: AddGroupMember) -> Self {
        b.build()
    }
}

impl IntoBatchOp for AddGroupMember {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Remove a user from a group. Returns a builder (HMAC-gated, see
/// [`RemoveGroupMember::hmac`]).
pub fn remove_group_member(group: GroupRef, user: u64) -> RemoveGroupMember {
    RemoveGroupMember {
        group,
        user,
        hmac: None,
    }
}

/// Builder for [`RemoveGroupMemberOp`].
pub struct RemoveGroupMember {
    group: GroupRef,
    user: u64,
    hmac: Option<String>,
}

impl RemoveGroupMember {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_remove_group_member(group, user)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RemoveGroupMember(RemoveGroupMemberOp {
            remove_group_member: self.group,
            user: self.user,
            hmac: self.hmac,
        })
    }
}

impl From<RemoveGroupMember> for BatchOp {
    fn from(b: RemoveGroupMember) -> Self {
        b.build()
    }
}

impl IntoBatchOp for RemoveGroupMember {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
