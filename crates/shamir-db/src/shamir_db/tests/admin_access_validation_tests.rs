//! Tests for task #543 (admin-ddl #5 / identity-session #5).
//!
//! Original scope: `chown` / `chgrp` / `add_group_member` should validate
//! that the target id resolves to a real, currently-existing
//! principal/group before writing it into the catalogue, plus `chown`
//! should forbid handing a resource to `OWNER_SYSTEM` unless the acting
//! actor already is `Actor::System`.
//!
//! **Scoped down after landing**: a full `--full` test run across
//! `shamir-db` surfaced 14+ pre-existing test failures once the
//! id-must-exist checks were in place (`chown(path, 7)`,
//! `chgrp(path, Some(3))`, `add_group_member(GroupRef::Name{..}, 20)` —
//! all with NO prior `create_user`/`create_group` call). This codebase's
//! established, pervasive convention treats a numeric owner/group/member
//! id as a valid, free-standing identifier that need NOT correspond to a
//! persisted record — enforcing existence broke that convention across
//! `shamir-db` AND `shamir-server` (e.g.
//! `hmac_gate.rs::chgrp_with_correct_hmac_accepted` chgrps to a literal
//! group id with no `create_group`). Reconciling "should an owner/member
//! id be required to exist" with the actual identity model is task
//! #548/#549's job (principal ids are non-cryptographic hashes of a
//! mutable username; there are two desynced user/role stores) — not
//! something to force through here.
//!
//! What THIS task ends up closing (the part that didn't conflict with
//! any existing convention):
//! - `chown` to `OWNER_SYSTEM` (`0`) by a non-System actor is rejected
//!   (a one-way footgun/DoS lockout — only `Actor::System` could manage
//!   the resource afterwards). `chown` to any OTHER id — existing or
//!   not — is unconditionally allowed, matching the codebase's existing
//!   convention.
//! - `add_group_member` against a `GroupRef::Id` that resolves to no
//!   persisted group is rejected — `resolve_group_id` only validates
//!   `GroupRef::Name` (a scan), and `GroupRef::Id` passed through
//!   unchecked would let `system_store::add_group_member` silently
//!   fabricate a phantom group record at the dangling id. This is a
//!   narrower, genuinely orthogonal gap: no pre-existing test relies on
//!   `add_group_member(GroupRef::Id{..a nonexistent id..}, _)` succeeding
//!   (every existing caller either uses `GroupRef::Name` after a real
//!   `create_group`, or calls the underlying `ShamirDb::add_group_member`
//!   method directly, which this fix does not touch).
//!
//! A properly-scoped follow-up for the reverted "does this owner/member
//! id resolve to a real principal" validation is left for once
//! #548/#549 settle the identity model.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;
use shamir_types::access::{principal_id, Actor, ResourceMeta, ResourcePath, OWNER_SYSTEM};

/// In-memory `ShamirDb` with `testdb` / `main` / `items`. Uses
/// `ShamirDb::add_repo` (not `DbInstance::add_repo`) so the table
/// catalogue record is persisted to the system store — `resource_meta`
/// / `set_resource_meta` for a `Table` path require the catalogue record
/// to exist (see `access_tree_tests.rs`'s `setup` for the same pattern).
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", repo_config).await.unwrap();
    shamir
}

/// Persist a real user record via the wire-facing `create_user` admin op
/// (same shape used by `create_user_emits_system_changefeed_event`), and
/// return its derived `principal_id`.
async fn seed_user(shamir: &ShamirDb, name: &str) -> u64 {
    let mut b = Batch::new();
    b.id(1);
    b.create_user("cu", ddl::create_user(name, "correct horse battery staple"));
    let req = b.to_request_via_msgpack();
    shamir
        .execute("testdb", &req)
        .await
        .unwrap_or_else(|e| panic!("seed_user({name}) failed: {e:?}"));
    principal_id(name)
}

// ============================================================================
// chown
// ============================================================================

/// Documents the (deliberate, unchanged) convention: `chown` to a user id
/// with no persisted user record still succeeds — this codebase does not
/// require an owner id to resolve to a real principal (see module doc).
#[tokio::test]
async fn chown_to_nonexistent_user_still_succeeds() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.chown(
        "co",
        ddl::chown(ddl::res::table("testdb", "main", "items"), 999_999),
    );
    let req = b.to_request_via_msgpack();

    shamir
        .execute("testdb", &req)
        .await
        .expect("chown to a numeric id with no persisted user must still succeed");

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "main", "items"))
        .await
        .unwrap();
    assert_eq!(meta.owner, Actor::User(999_999));
}

/// `chown` to `OWNER_SYSTEM` (`0`) by a non-System actor must be rejected
/// — a one-way footgun/DoS lockout (only `Actor::System` could manage the
/// resource afterwards).
#[tokio::test]
async fn chown_to_system_by_non_system_actor_is_rejected() {
    let shamir = setup().await;
    let alice = seed_user(&shamir, "alice").await;

    // Ancestors (database, store) default to enforced/System-owned — open
    // them so alice's ownership of the *table* is the sole gate under
    // test (mirrors the pattern in facade_gateway_acl_tests.rs).
    let open = ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "main"), &open)
        .await
        .unwrap();

    // Make alice the owner + grant her Manage so she can reach the
    // chown handler at all.
    let meta = ResourceMeta {
        owner: Actor::User(alice),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.chown(
        "co",
        ddl::chown(ddl::res::table("testdb", "main", "items"), OWNER_SYSTEM),
    );
    let req = b.to_request_via_msgpack();

    let err = shamir
        .execute_as(Actor::User(alice), "testdb", &req)
        .await
        .expect_err("chown to OWNER_SYSTEM by a non-System actor must be rejected");
    assert_eq!(err.code(), Some("invalid_owner"));

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "main", "items"))
        .await
        .unwrap();
    assert_eq!(
        meta.owner,
        Actor::User(alice),
        "resource must still be owned by alice, not System"
    );
}

/// `chown` to `OWNER_SYSTEM` by `Actor::System` itself must still succeed
/// — this is the legitimate path (System reassigning a resource to
/// itself / an admin root session making a resource System-owned).
#[tokio::test]
async fn chown_to_system_by_system_actor_succeeds() {
    let shamir = setup().await;
    let alice = seed_user(&shamir, "alice").await;

    let meta = ResourceMeta {
        owner: Actor::User(alice),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.chown(
        "co",
        ddl::chown(ddl::res::table("testdb", "main", "items"), OWNER_SYSTEM),
    );
    let req = b.to_request_via_msgpack();

    // `execute` = `execute_as(Actor::System, ...)`.
    shamir
        .execute("testdb", &req)
        .await
        .expect("System actor chowning a resource to System must succeed");

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "main", "items"))
        .await
        .unwrap();
    assert_eq!(meta.owner, Actor::System);
}

/// Happy path: `chown` to a real, existing user id succeeds and the
/// meta reflects the new owner.
#[tokio::test]
async fn chown_to_existing_user_succeeds() {
    let shamir = setup().await;
    let bob = seed_user(&shamir, "bob").await;

    let mut b = Batch::new();
    b.id(1);
    b.chown(
        "co",
        ddl::chown(ddl::res::table("testdb", "main", "items"), bob),
    );
    let req = b.to_request_via_msgpack();

    shamir
        .execute("testdb", &req)
        .await
        .expect("chown to an existing user must succeed");

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "main", "items"))
        .await
        .unwrap();
    assert_eq!(meta.owner, Actor::User(bob));
}

// ============================================================================
// chgrp
// ============================================================================

/// Documents the (deliberate, unchanged) convention: `chgrp` to a group id
/// with no persisted group record still succeeds (mirrors `chown`'s
/// scoped-down decision above — see module doc).
#[tokio::test]
async fn chgrp_to_nonexistent_group_still_succeeds() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.chgrp(
        "cg",
        ddl::chgrp(ddl::res::table("testdb", "main", "items"), Some(999_999)),
    );
    let req = b.to_request_via_msgpack();

    shamir
        .execute("testdb", &req)
        .await
        .expect("chgrp to a numeric id with no persisted group must still succeed");

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "main", "items"))
        .await
        .unwrap();
    assert_eq!(meta.group, Some(999_999));
}

/// `chgrp` to `None` (clearing the group) needs no validation and must
/// always succeed, even with no groups created at all.
#[tokio::test]
async fn chgrp_to_none_clears_unconditionally() {
    let shamir = setup().await;
    let gid = shamir.create_group("devs").await.unwrap();

    // First set a real group so there is something to clear.
    let meta = ResourceMeta {
        owner: Actor::System,
        group: Some(gid),
        mode: 0o755,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.chgrp(
        "cg",
        ddl::chgrp(ddl::res::table("testdb", "main", "items"), None),
    );
    let req = b.to_request_via_msgpack();

    shamir
        .execute("testdb", &req)
        .await
        .expect("chgrp to None (clear) must always succeed");

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "main", "items"))
        .await
        .unwrap();
    assert_eq!(meta.group, None);
}

/// Happy path: `chgrp` to a real, existing group id succeeds.
#[tokio::test]
async fn chgrp_to_existing_group_succeeds() {
    let shamir = setup().await;
    let gid = shamir.create_group("devs").await.unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.chgrp(
        "cg",
        ddl::chgrp(ddl::res::table("testdb", "main", "items"), Some(gid)),
    );
    let req = b.to_request_via_msgpack();

    shamir
        .execute("testdb", &req)
        .await
        .expect("chgrp to an existing group must succeed");

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "main", "items"))
        .await
        .unwrap();
    assert_eq!(meta.group, Some(gid));
}

// ============================================================================
// add_group_member
// ============================================================================

/// `add_group_member` against a `GroupRef::Id` that resolves to no
/// persisted group must be rejected. `resolve_group_id` only validates
/// existence for `GroupRef::Name` (it scans `load_groups()`); the `Id`
/// variant passes the id straight through with no check, so without an
/// explicit group-existence check here `system_store::add_group_member`
/// would silently fabricate a phantom group record for the dangling id.
/// This is the one existence check task #543 keeps — see module doc for
/// why the group-side check survives while the user-side one was
/// reverted.
#[tokio::test]
async fn add_group_member_nonexistent_group_id_is_rejected() {
    let shamir = setup().await;
    let alice = seed_user(&shamir, "alice").await;

    let mut b = Batch::new();
    b.id(1);
    b.add_group_member(
        "agm",
        ddl::add_group_member(ddl::GroupRef::Id { id: 999_999 }, alice),
    );
    let req = b.to_request_via_msgpack();

    let err = shamir
        .execute("testdb", &req)
        .await
        .expect_err("add_group_member against a nonexistent group id must be rejected");
    assert_eq!(err.code(), Some("invalid_owner"));

    // No phantom group record must have been fabricated at the dangling id.
    let group = shamir.system_store().load_group(999_999).await.unwrap();
    assert!(
        group.is_none(),
        "a phantom group record must not have been created for the dangling id"
    );
}

/// Documents the (deliberate, unchanged) convention: `add_group_member`
/// with a numeric user id that has no persisted user record still
/// succeeds (mirrors `chown`/`chgrp`'s scoped-down decision — see module
/// doc). The GROUP side is still validated (see the test above); only
/// the USER side's existence check was reverted.
#[tokio::test]
async fn add_group_member_nonexistent_user_still_succeeds() {
    let shamir = setup().await;
    let gid = shamir.create_group("devs").await.unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.add_group_member(
        "agm",
        ddl::add_group_member(ddl::GroupRef::Id { id: gid }, 999_999),
    );
    let req = b.to_request_via_msgpack();

    shamir
        .execute("testdb", &req)
        .await
        .expect("add_group_member with a numeric id with no persisted user must still succeed");

    let members = shamir.group_members(gid).await.unwrap();
    assert!(members.contains(&999_999));
}

/// Happy path: `add_group_member` with a real, existing user id succeeds
/// (don't regress the existing group-membership behaviour).
#[tokio::test]
async fn add_group_member_existing_user_succeeds() {
    let shamir = setup().await;
    let gid = shamir.create_group("devs").await.unwrap();
    let alice = seed_user(&shamir, "alice").await;

    let mut b = Batch::new();
    b.id(1);
    b.add_group_member(
        "agm",
        ddl::add_group_member(ddl::GroupRef::Id { id: gid }, alice),
    );
    let req = b.to_request_via_msgpack();

    shamir
        .execute("testdb", &req)
        .await
        .expect("add_group_member with an existing user must succeed");

    let members = shamir.group_members(gid).await.unwrap();
    assert!(members.contains(&alice));
}

// ============================================================================
// remove_group_member (decision: harmless no-op, left unvalidated)
// ============================================================================

/// `remove_group_member` for a user id that was never a member (whether
/// or not it resolves to a real user) is a harmless, idempotent no-op —
/// it must succeed rather than error, since nothing dangling is ever
/// written.
#[tokio::test]
async fn remove_group_member_nonexistent_user_is_harmless_noop() {
    let shamir = setup().await;
    let gid = shamir.create_group("devs").await.unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.remove_group_member(
        "rgm",
        ddl::remove_group_member(ddl::GroupRef::Id { id: gid }, 999_999),
    );
    let req = b.to_request_via_msgpack();

    shamir
        .execute("testdb", &req)
        .await
        .expect("remove_group_member for a non-member id must be a harmless no-op");

    let members = shamir.group_members(gid).await.unwrap();
    assert!(!members.contains(&999_999));
}
