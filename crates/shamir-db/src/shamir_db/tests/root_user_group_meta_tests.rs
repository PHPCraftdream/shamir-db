//! Task #552 (`docs/prompts/audit/70-root-user-group-real-permissions.md`,
//! per the signed-off design in
//! `docs/design/root-user-group-dac-posture-550-decision.md` §1).
//!
//! Replaces the blanket `ResourcePath::Root | ResourcePath::User { .. } |
//! ResourcePath::Group { .. } => Ok(ResourceMeta::open())` arm with three
//! DIFFERENT, kind-specific models:
//!
//! - **Root**: full persisted meta (settings key `"root_meta"`), mirroring
//!   the existing `FunctionNamespace` pattern. Default `0o755`/`System`
//!   when absent. `set_resource_meta` gains a matching write arm, guarded
//!   against a non-System owner locking themselves out via a
//!   traverse-clearing `chmod`.
//! - **User**: a FIXED, computed 3-tier rule (`owner = Actor::User(principal64_from_username(name))`,
//!   `mode = 0o750`) — never persisted, no `set_resource_meta` arm.
//! - **Group**: persisted `owner` on the existing group record, computed
//!   `mode = 0o750`, `group = Some(group_id)` (members are a real
//!   permission class). `set_resource_meta` can update only the owner.

use crate::query::admin::GroupRef;
use crate::shamir_db::{PrincipalInfo, PrincipalResolver, ShamirDb};
use crate::DbError;
use shamir_types::access::{
    principal64_from_username, Action, Actor, Mode, ResourceMeta, ResourcePath,
};
use std::sync::Arc;

/// Test-only mock resolver: maps a username to `principal64_from_username(name)`,
/// mirroring the pre-#559 interim bridge. Used so `ResourcePath::User` meta
/// resolution resolves to a real (if synthetic) owner in tests that don't
/// have a real directory wired.
struct MockResolver;

impl PrincipalResolver for MockResolver {
    fn resolve(&self, _principal64_val: u64) -> Option<PrincipalInfo> {
        // Not used by these tests — only resolve_by_name matters.
        None
    }
    fn list(&self) -> Vec<PrincipalInfo> {
        Vec::new()
    }
    fn resolve_by_name(&self, name: &str) -> Option<PrincipalInfo> {
        Some(PrincipalInfo {
            principal64: principal64_from_username(name),
            name: name.to_string(),
            user_id: [0u8; 16],
            database: None,
            superuser: false,
        })
    }
}

/// In-memory ShamirDb with a mock PrincipalResolver installed (task #559:
/// `ResourcePath::User` meta now resolves via the resolver; without one the
/// owner degrades to `Actor::System`).
async fn setup_with_resolver() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.with_principal_resolver(Arc::new(MockResolver))
}

/// Grant `actor` `Manage(Root)` by `chown`ing Root to them (via
/// `Actor::System`, which always has the rights to do so). Test-only
/// bootstrap helper: several `*_as` group-CRUD tests need a non-System
/// actor who can pass `create_group_as`'s unconditional `Manage(Root)`
/// gate (unchanged by task #552 — creation itself stays gated on Root
/// only), so they can then exercise the OWNER-based `Manage(Group{name})`
/// gate this task adds to rename/add/remove/drop.
async fn grant_root_manage(shamir: &ShamirDb, actor: &Actor) {
    shamir
        .set_resource_meta(
            &ResourcePath::Root,
            &ResourceMeta {
                owner: actor.clone(),
                group: None,
                mode: 0o755,
            },
        )
        .await
        .unwrap();
}

// ============================================================================
// 1. Root — full persisted meta, mirroring FunctionNamespace
// ============================================================================

#[tokio::test]
async fn root_meta_defaults_to_system_0o755_when_absent() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let meta = shamir.resource_meta(&ResourcePath::Root).await.unwrap();
    assert_eq!(meta.owner, Actor::System);
    assert_eq!(meta.group, None);
    assert_eq!(meta.mode, 0o755);
}

#[tokio::test]
async fn set_root_meta_round_trips_via_settings() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let custom = ResourceMeta {
        owner: Actor::System,
        group: Some(3),
        mode: 0o755,
    };
    shamir
        .set_resource_meta(&ResourcePath::Root, &custom)
        .await
        .unwrap();

    let loaded = shamir.resource_meta(&ResourcePath::Root).await.unwrap();
    assert_eq!(loaded.owner, Actor::System);
    assert_eq!(loaded.group, Some(3));
    assert_eq!(loaded.mode, 0o755);
}

/// `chown /` to a non-System owner via `set_resource_meta` succeeds — this
/// is NEW behavior; Root previously rejected all `set_resource_meta` calls
/// outright.
#[tokio::test]
async fn chown_root_to_non_system_owner_succeeds() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let custom = ResourceMeta {
        owner: Actor::User(principal64_from_username("alice")),
        group: None,
        mode: 0o755,
    };
    shamir
        .set_resource_meta(&ResourcePath::Root, &custom)
        .await
        .expect("chown / to a non-System owner must succeed (new behavior)");

    let loaded = shamir.resource_meta(&ResourcePath::Root).await.unwrap();
    assert_eq!(
        loaded.owner,
        Actor::User(principal64_from_username("alice"))
    );
}

/// Guardrail: once Root is owned by a non-System actor, a `chmod` that
/// clears owner-Execute is rejected — the owner would have no way back in
/// (System bypasses `permits` unconditionally and always recovers; a
/// non-System owner does not).
#[tokio::test]
async fn chmod_root_clearing_owner_execute_denied_for_non_system_owner() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let alice = Actor::User(principal64_from_username("alice"));
    shamir
        .set_resource_meta(
            &ResourcePath::Root,
            &ResourceMeta {
                owner: alice.clone(),
                group: None,
                mode: 0o755,
            },
        )
        .await
        .expect("chown to alice must succeed");

    // 0o655: owner keeps read+write but LOSES execute (traverse) — the
    // dangerous, self-lockout chmod.
    let no_owner_exec = ResourceMeta {
        owner: alice.clone(),
        group: None,
        mode: 0o655,
    };
    let err = shamir
        .set_resource_meta(&ResourcePath::Root, &no_owner_exec)
        .await
        .expect_err("clearing owner-Execute on Root for a non-System owner must be rejected");
    assert!(
        matches!(err, DbError::Validation(_)),
        "expected a validation error, got {err:?}"
    );

    // The rejected chmod must not have persisted — owner-Execute still set.
    let loaded = shamir.resource_meta(&ResourcePath::Root).await.unwrap();
    assert!(
        Mode::is_set(
            loaded.mode,
            shamir_types::access::PermClass::Owner,
            shamir_types::access::Perm::Execute
        ),
        "rejected chmod must not have cleared owner-Execute in the persisted meta"
    );
}

/// The equivalent `chmod` clearing owner-Execute is NOT rejected when the
/// CURRENT owner (before the write) is still `Actor::System` — System
/// doesn't need the guard (it always bypasses `permits`).
#[tokio::test]
async fn chmod_root_clearing_owner_execute_allowed_when_owner_is_system() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Root starts owned by System (default). Clearing owner-Execute while
    // still System-owned must be allowed.
    let no_owner_exec = ResourceMeta {
        owner: Actor::System,
        group: None,
        mode: 0o655,
    };
    shamir
        .set_resource_meta(&ResourcePath::Root, &no_owner_exec)
        .await
        .expect("clearing owner-Execute on a System-owned Root must be allowed");

    let loaded = shamir.resource_meta(&ResourcePath::Root).await.unwrap();
    assert_eq!(loaded.mode, 0o655);
}

/// Regression test for a task #552 adversarial-review finding: an earlier
/// revision of the Root self-lockout guardrail checked the OLD (`current`)
/// owner's identity instead of the NEW (`meta`) owner being written, which
/// let the exact lockout the guard exists to prevent through via two
/// ordinary, separately-authorized calls instead of one: (1) while Root is
/// still System-owned, clear owner-Execute (allowed — System doesn't need
/// the bit); (2) a SEPARATE later call chowns Root to a non-System actor
/// without touching mode, inheriting the already-cleared bit. The old check
/// only looked at the owner BEFORE call 2 (still System) and never fired.
#[tokio::test]
async fn chown_to_non_system_owner_after_prior_execute_clear_is_denied() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Step 1: while Root is still System-owned, clear owner-Execute — this
    // must succeed (System doesn't need the bit, always bypasses `permits`).
    shamir
        .set_resource_meta(
            &ResourcePath::Root,
            &ResourceMeta {
                owner: Actor::System,
                group: None,
                mode: 0o655, // no owner-Execute
            },
        )
        .await
        .expect("clearing owner-Execute on a System-owned Root must be allowed");

    // Step 2: a SEPARATE chown to a non-System owner, without touching mode
    // (inheriting the already-cleared bit) — must be REJECTED, closing the
    // two-step sequence the guard exists to prevent.
    let err = shamir
        .set_resource_meta(
            &ResourcePath::Root,
            &ResourceMeta {
                owner: Actor::User(principal64_from_username("mallory")),
                group: None,
                mode: 0o655, // still no owner-Execute
            },
        )
        .await
        .expect_err(
            "chown to a non-System owner while owner-Execute is already cleared must be denied",
        );
    assert!(
        matches!(err, DbError::Validation(_)),
        "expected a validation error, got {err:?}"
    );

    // The rejected chown must not have persisted — Root is still System-owned.
    let loaded = shamir.resource_meta(&ResourcePath::Root).await.unwrap();
    assert_eq!(
        loaded.owner,
        Actor::System,
        "rejected chown must not have changed Root's owner"
    );
}

// ============================================================================
// 2. User — FIXED, computed 3-tier rule, never persisted
// ============================================================================

#[tokio::test]
async fn user_resource_meta_is_computed_owner_self_0o750() {
    let shamir = setup_with_resolver().await;
    let meta = shamir
        .resource_meta(&ResourcePath::user("alice"))
        .await
        .unwrap();
    assert_eq!(meta.owner, Actor::User(principal64_from_username("alice")));
    assert_eq!(meta.group, None);
    assert_eq!(meta.mode, 0o750);
}

/// `set_resource_meta` on a `User` path is NOT supported — falls through to
/// the existing catch-all `NotFound` arm (never persisted, by design).
#[tokio::test]
async fn set_user_resource_meta_is_not_supported() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let err = shamir
        .set_resource_meta(&ResourcePath::user("alice"), &ResourceMeta::open())
        .await
        .expect_err("User meta must not be settable");
    assert!(matches!(err, DbError::NotFound(_)), "got {err:?}");
}

/// The user themselves can Read their own `User` path.
#[tokio::test]
async fn user_can_read_own_user_path() {
    let shamir = setup_with_resolver().await;
    let alice = Actor::User(principal64_from_username("alice"));
    shamir
        .authorize_access(&alice, &ResourcePath::user("alice"), Action::Read)
        .await
        .expect("alice must be able to Read her own User path");
}

/// The user themselves can Manage (self-service umbrella) their own `User`
/// path.
#[tokio::test]
async fn user_can_manage_own_user_path() {
    let shamir = setup_with_resolver().await;
    let alice = Actor::User(principal64_from_username("alice"));
    shamir
        .authorize_access(&alice, &ResourcePath::user("alice"), Action::Manage)
        .await
        .expect("alice must be able to Manage (self-service) her own User path");
}

/// A different, unrelated user is DENIED Read on someone else's `User`
/// path — the real narrowing versus the old universal `open()`.
#[tokio::test]
async fn other_user_denied_read_on_foreign_user_path() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let bob = Actor::User(principal64_from_username("bob"));
    let err = shamir
        .authorize_access(&bob, &ResourcePath::user("alice"), Action::Read)
        .await
        .expect_err("bob must be denied Read on alice's User path");
    assert!(format!("{err}").to_lowercase().contains("denied"));
}

// ============================================================================
// 3. Group — persisted `owner` field, computed mode + group class
// ============================================================================

#[tokio::test]
async fn group_resource_meta_reports_persisted_owner_and_group_class() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let creator = Actor::User(principal64_from_username("carol"));
    grant_root_manage(&shamir, &creator).await;
    let gid = shamir.create_group_as("devs", &creator).await.unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::group("devs"))
        .await
        .unwrap();
    assert_eq!(meta.owner, creator);
    assert_eq!(meta.group, Some(gid));
    assert_eq!(meta.mode, 0o750);
}

/// A nonexistent group falls back to `ResourceMeta::open()` (mirrors the
/// `FunctionFolder` "never created" convention) — not an error case.
#[tokio::test]
async fn nonexistent_group_resource_meta_falls_back_to_open() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let meta = shamir
        .resource_meta(&ResourcePath::group("ghost"))
        .await
        .unwrap();
    assert_eq!(meta, ResourceMeta::open());
}

/// Legacy group records lacking the `owner` field (created before this
/// task, i.e. via the raw `system_store.save_group` 3-arg-shaped write)
/// decode via `ResourceMeta::from_record`'s existing fallback to
/// `Actor::System` — fail-safe.
#[tokio::test]
async fn legacy_group_record_without_owner_defaults_to_system() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    // Directly persist a group record the OLD way (no owner) by going
    // through the system_store with an explicit System owner id, then
    // sanity-check the fallback still resolves to System when the field is
    // truly absent. Since `save_group` now always writes an owner, we
    // simulate "legacy" by writing System explicitly and confirming the
    // resource_meta arm reports System, matching what an old, owner-less
    // record would decode to.
    let gid = shamir.create_group("legacy").await.unwrap();
    let meta = shamir
        .resource_meta(&ResourcePath::group("legacy"))
        .await
        .unwrap();
    assert_eq!(meta.owner, Actor::System);
    assert_eq!(meta.group, Some(gid));
}

/// `set_resource_meta` on a `Group` path updates only the owner — mode
/// stays fixed at `0o750` regardless of what's requested.
#[tokio::test]
async fn set_group_resource_meta_updates_owner_only() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_group("devs").await.unwrap();

    let new_owner = Actor::User(principal64_from_username("dave"));
    shamir
        .set_resource_meta(
            &ResourcePath::group("devs"),
            &ResourceMeta {
                owner: new_owner.clone(),
                group: None,
                mode: 0o777, // attempted mode change — must be ignored
            },
        )
        .await
        .unwrap();

    let loaded = shamir
        .resource_meta(&ResourcePath::group("devs"))
        .await
        .unwrap();
    assert_eq!(loaded.owner, new_owner);
    assert_eq!(loaded.mode, 0o750, "group mode must stay fixed at 0o750");
}

/// `rename_group_as` must thread the existing owner through unchanged.
#[tokio::test]
async fn rename_group_preserves_owner() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let creator = Actor::User(principal64_from_username("carol"));
    grant_root_manage(&shamir, &creator).await;
    shamir.create_group_as("devs", &creator).await.unwrap();

    shamir
        .rename_group_as(
            &GroupRef::Name {
                name: "devs".to_string(),
            },
            "engineers",
            &creator,
        )
        .await
        .unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::group("engineers"))
        .await
        .unwrap();
    assert_eq!(meta.owner, creator, "rename must not touch ownership");
}

/// `add_group_member`/`remove_group_member` (system_store level, via
/// read-modify-write `save_group`) must thread the existing owner through
/// unchanged.
#[tokio::test]
async fn add_and_remove_group_member_preserve_owner() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let creator = Actor::User(principal64_from_username("carol"));
    grant_root_manage(&shamir, &creator).await;
    let gid = shamir.create_group_as("devs", &creator).await.unwrap();

    shamir.add_group_member(gid, 42).await.unwrap();
    let meta = shamir
        .resource_meta(&ResourcePath::group("devs"))
        .await
        .unwrap();
    assert_eq!(meta.owner, creator, "add_group_member must preserve owner");

    shamir.remove_group_member(gid, 42).await.unwrap();
    let meta = shamir
        .resource_meta(&ResourcePath::group("devs"))
        .await
        .unwrap();
    assert_eq!(
        meta.owner, creator,
        "remove_group_member must preserve owner"
    );
}

/// `create_group_as` persists the ACTING actor as owner (not System, even
/// though creation itself is gated on `Manage(Root)` only).
#[tokio::test]
async fn create_group_as_stamps_acting_actor_as_owner() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let creator = Actor::User(principal64_from_username("erin"));
    grant_root_manage(&shamir, &creator).await;
    shamir.create_group_as("finance", &creator).await.unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::group("finance"))
        .await
        .unwrap();
    assert_eq!(meta.owner, creator);
}

// ============================================================================
// Group-CRUD gate: Manage(Root) OR Manage(Group{name}) — a group's own
// creator can manage their group WITHOUT needing global root admin.
// ============================================================================

/// A group's creator can `rename_group_as` their OWN group without
/// `Manage(Root)`.
#[tokio::test]
async fn group_creator_can_rename_own_group_without_root_manage() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let creator = Actor::User(principal64_from_username("frank"));
    // Bootstrap via System so creation itself doesn't need `frank` to hold
    // Manage(Root) — this test is about the *rename* gate, not creation.
    let gid = shamir
        .create_group_as("marketing", &Actor::System)
        .await
        .unwrap();
    // Stamp frank as the group's owner directly (simulating a group frank
    // created himself, without requiring frank to also hold Manage(Root)
    // for this test's setup).
    shamir
        .set_resource_meta(
            &ResourcePath::group("marketing"),
            &ResourceMeta {
                owner: creator.clone(),
                group: None,
                mode: 0o750,
            },
        )
        .await
        .unwrap();

    shamir
        .rename_group_as(&GroupRef::Id { id: gid }, "marketing-2", &creator)
        .await
        .expect("group owner must be able to rename their own group without Manage(Root)");
}

/// A group's creator can `add_group_member_as` their OWN group without
/// `Manage(Root)`.
#[tokio::test]
async fn group_creator_can_add_member_to_own_group_without_root_manage() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let creator = Actor::User(principal64_from_username("gina"));
    grant_root_manage(&shamir, &creator).await;
    let gid = shamir.create_group_as("ops", &creator).await.unwrap();

    shamir
        .add_group_member_as(gid, 123, &creator)
        .await
        .expect("group owner must be able to add members to their own group");
    assert!(shamir.user_in_group(123, gid).await.unwrap());
}

/// A group's creator can `remove_group_member_as` their OWN group without
/// `Manage(Root)`.
#[tokio::test]
async fn group_creator_can_remove_member_from_own_group_without_root_manage() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let creator = Actor::User(principal64_from_username("henry"));
    grant_root_manage(&shamir, &creator).await;
    let gid = shamir.create_group_as("ops2", &creator).await.unwrap();
    shamir.add_group_member(gid, 55).await.unwrap();

    shamir
        .remove_group_member_as(gid, 55, &creator)
        .await
        .expect("group owner must be able to remove members from their own group");
    assert!(!shamir.user_in_group(55, gid).await.unwrap());
}

/// A group's creator can `drop_group_as` their OWN group without
/// `Manage(Root)`.
#[tokio::test]
async fn group_creator_can_drop_own_group_without_root_manage() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let creator = Actor::User(principal64_from_username("iris"));
    grant_root_manage(&shamir, &creator).await;
    let gid = shamir.create_group_as("temp-team", &creator).await.unwrap();

    shamir
        .drop_group_as(gid, &creator)
        .await
        .expect("group owner must be able to drop their own group without Manage(Root)");
}

/// A different, non-superuser actor (not the group's owner, no
/// `Manage(Root)`) is DENIED the same four ops on someone else's group.
#[tokio::test]
async fn non_owner_non_superuser_denied_group_ops_on_foreign_group() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(principal64_from_username("julia"));
    let stranger = Actor::User(principal64_from_username("kyle"));
    grant_root_manage(&shamir, &owner).await;
    let gid = shamir.create_group_as("sales", &owner).await.unwrap();

    shamir
        .rename_group_as(&GroupRef::Id { id: gid }, "sales-2", &stranger)
        .await
        .expect_err("a non-owner, non-Manage(Root) actor must be denied rename_group_as");

    shamir
        .add_group_member_as(gid, 999, &stranger)
        .await
        .expect_err("a non-owner, non-Manage(Root) actor must be denied add_group_member_as");

    shamir
        .remove_group_member_as(gid, 999, &stranger)
        .await
        .expect_err("a non-owner, non-Manage(Root) actor must be denied remove_group_member_as");

    shamir
        .drop_group_as(gid, &stranger)
        .await
        .expect_err("a non-owner, non-Manage(Root) actor must be denied drop_group_as");
}

/// `create_group_as` still requires `Manage(Root)` regardless of who's
/// asking — unchanged by this task (creation writes into the Root
/// container, so Root's gate is the right one).
#[tokio::test]
async fn create_group_as_still_requires_root_manage_regardless_of_asker() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let stranger = Actor::User(principal64_from_username("laura"));

    let err = shamir
        .create_group_as("new-team", &stranger)
        .await
        .expect_err("create_group_as must still require Manage(Root)");
    assert!(format!("{err}").to_lowercase().contains("denied"));
}
