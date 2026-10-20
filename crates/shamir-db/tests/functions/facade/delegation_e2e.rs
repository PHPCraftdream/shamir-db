//!
//! Two concerns proven against the real `execute_as` → `authorize_access`
//! path (no flags, no bypass except `Actor::System`):
//!
//!   A. **Closed hole.** A non-privileged `Actor::User` can no longer
//!      create users / groups / grant roles. `Actor::System` still can.
//!   B. **Owner delegation.** The owner-delegation authorization gate
//!      correctly denies non-owners and wrong-scope callers (negative), and
//!      — with a `UserAdminPort` installed — a real database owner
//!      (non-superuser `Actor::User`) can create a user scoped to their own
//!      database end-to-end (positive, task #559). The wire-level version of
//!      this positive path is architecturally unreachable: a coarse
//!      admin/auth gate added by task #553 rejects any admin op from a
//!      non-superuser session before it reaches `authorize_user_lifecycle` —
//!      so this proof is written directly against `execute_as`, which
//!      bypasses that wire-level gate, exactly as the negative-gate tests
//!      below already do.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::auth::CreateUserOp;
use shamir_db::query::batch::BatchRequest;
use shamir_db::{PortError, ShamirDb, UserAdminPort};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

/// Build a ShamirDb with database "testdb"/repo "main"/table "users".
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Build a single-op batch request keyed by alias "op".
fn one_op(op: impl shamir_query_builder::batch::IntoBatchOp) -> BatchRequest {
    let mut b = Batch::new();
    b.id(1);
    b.op("op", op);
    b.to_request_via_msgpack()
}

// ===========================================================================
// Serde — CreateUserOp round-trips the optional `database` scope
// ===========================================================================

#[test]
fn create_user_op_serde_round_trip_with_database() {
    let op = CreateUserOp {
        create_user: "bob".to_string(),
        password: "pw".to_string().into(),
        roles: vec!["readonly".to_string()],
        database: Some("testdb".to_string()),
        hmac: None,
    };
    assert_eq!(op.create_user, "bob");
    assert_eq!(op.database.as_deref(), Some("testdb"));

    // Serialize via msgpack → deserialize must preserve the scope.
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let op2: CreateUserOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op2.database, op.database);
}

#[test]
fn create_user_op_serde_omits_database_when_absent() {
    let op = CreateUserOp {
        create_user: "bob".to_string(),
        password: "pw".to_string().into(),
        roles: vec![],
        database: None,
        hmac: None,
    };
    assert_eq!(op.database, None);

    // skip_serializing_if = Option::is_none → no key when unset.
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let op2: CreateUserOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op2.database, None);
}

// ===========================================================================
// A. Closed hole — non-privileged users cannot run user/role/group admin ops
// ===========================================================================

#[tokio::test]
async fn non_admin_cannot_create_user() {
    let shamir = setup().await;

    // Database is open (0o777) so `execute_as` Read-gate on the db passes,
    // proving the denial comes from the user-lifecycle check, not traversal.
    let req = one_op(ddl::create_user("mallory", "pw"));
    let resp = shamir.execute_as(Actor::User(7), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "non-admin user must NOT be able to create a user: {resp:?}"
    );
}

#[tokio::test]
async fn non_admin_cannot_create_group() {
    let shamir = setup().await;

    let req = one_op(ddl::create_group("devs"));
    let resp = shamir.execute_as(Actor::User(7), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "non-admin user must NOT be able to create a group: {resp:?}"
    );
}

/// A non-admin user must NOT be able to grant a role — the `Manage(Root)`
/// authorization gate denies before the `UserAdminPort` is even consulted.
#[tokio::test]
async fn non_admin_cannot_grant_role() {
    let shamir = setup().await;

    let req = one_op(ddl::grant_role("analyst", "some_user"));
    let resp = shamir.execute_as(Actor::User(7), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "non-admin user must NOT be able to grant a role: {resp:?}"
    );
}

/// `Actor::System` still drives admin ops (bypass). Group creation is
/// tested here as a representative; user/role creation now routes through
/// the `UserAdminPort` (task #559) and is covered by shamir-server tests.
#[tokio::test]
async fn system_can_create_group() {
    let shamir = setup().await;

    let cg = one_op(ddl::create_group("devs"));
    assert!(
        shamir.execute("testdb", &cg).await.is_ok(),
        "System should create a group"
    );
}

// ===========================================================================
// B. Owner delegation — negative gate (the positive path needs a port)
// ===========================================================================

/// Make `actor` the owner of "testdb" (mode stays open so Read/Manage hold).
async fn make_db_owner(shamir: &ShamirDb, actor: Actor) {
    shamir
        .set_resource_meta(
            &ResourcePath::database("testdb"),
            &ResourceMeta {
                owner: actor,
                group: None,
                mode: 0o777,
            },
        )
        .await
        .unwrap();
}

/// A db owner must NOT create an unscoped (global) user — only a global
/// admin may. The authorization gate (`authorize_user_lifecycle(None)`)
/// denies before the port is consulted, so this holds even without a port
/// installed.
#[tokio::test]
async fn db_owner_cannot_create_unscoped_user() {
    let shamir = setup().await;
    let owner = Actor::User(1001);
    make_db_owner(&shamir, owner.clone()).await;

    // No `database` scope → only a global admin may create → denied.
    let req = one_op(ddl::create_user("global_user", "pw"));
    let resp = shamir.execute_as(owner.clone(), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "db owner must NOT create an unscoped (global) user: {resp:?}"
    );
}

/// A db owner must NOT create a user scoped to a database they do not own.
#[tokio::test]
async fn db_owner_cannot_create_user_scoped_to_other_db() {
    let shamir = setup().await;
    let owner = Actor::User(1001);
    make_db_owner(&shamir, owner.clone()).await;

    // Scope points at a database the actor does not own → denied.
    let req = one_op(ddl::create_user("intruder", "pw").database("otherdb"));
    let resp = shamir.execute_as(owner.clone(), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "db owner must NOT create a user scoped to a foreign db: {resp:?}"
    );
}

/// A non-owner with db Read access must still be denied scoped-user
/// creation (the delegation only admits the actual owner).
#[tokio::test]
async fn non_owner_with_db_read_still_cannot_create_scoped_user() {
    let shamir = setup().await;
    make_db_owner(&shamir, Actor::User(1001)).await;

    let req = one_op(ddl::create_user("x", "pw").database("testdb"));
    let resp = shamir.execute_as(Actor::User(2002), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "a non-owner must NOT create a scoped user even with db read: {resp:?}"
    );
}

/// Without a `UserAdminPort` installed, `create_user` returns
/// `not_supported` — the retirement of Store B is a hard cutover, not a
/// soft fallback.
#[tokio::test]
async fn create_user_without_port_returns_not_supported() {
    let shamir = setup().await;

    let req = one_op(ddl::create_user("alice", "pw"));
    let resp = shamir.execute("testdb", &req).await;
    assert!(
        resp.is_err(),
        "create_user without a port must fail: {resp:?}"
    );
    let err = resp.unwrap_err();
    assert_eq!(
        err.code(),
        Some("not_supported"),
        "expected code 'not_supported', got: {:?}",
        err.code()
    );
}

/// Without a `UserAdminPort`, `drop_user` also returns `not_supported`.
#[tokio::test]
async fn drop_user_without_port_returns_not_supported() {
    let shamir = setup().await;

    let req = one_op(ddl::drop_user("ghost"));
    let resp = shamir.execute("testdb", &req).await;
    assert!(resp.is_err());
    assert_eq!(resp.unwrap_err().code(), Some("not_supported"));
}

/// Without a `UserAdminPort`, `grant_role` returns `not_supported` (after
/// the `Manage(Root)` gate passes for System).
#[tokio::test]
async fn grant_role_without_port_returns_not_supported() {
    let shamir = setup().await;

    let req = one_op(ddl::grant_role("admin", "somebody"));
    let resp = shamir.execute("testdb", &req).await;
    assert!(resp.is_err());
    assert_eq!(resp.unwrap_err().code(), Some("not_supported"));
}

// ===========================================================================
// B (positive). Owner delegation with a port installed — a real database
// owner (non-superuser `Actor::User`) creates a user scoped to their own
// database, end-to-end through `UserAdminPort`.
// ===========================================================================

/// One recorded `create_user` call: `(name, roles, database)`.
type CreateUserCall = (String, Vec<String>, Option<String>);

/// Records every `create_user`/`drop_user` call it receives — proves the
/// call actually reached the port (not just that the authorization gate
/// admitted it).
#[derive(Default)]
struct RecordingPort {
    calls: Mutex<Vec<CreateUserCall>>,
    drop_calls: Mutex<Vec<String>>,
}

#[async_trait]
impl UserAdminPort for RecordingPort {
    async fn create_user(
        &self,
        name: &str,
        _password: &str,
        roles: Vec<String>,
        database: Option<String>,
    ) -> Result<[u8; 16], PortError> {
        self.calls
            .lock()
            .unwrap()
            .push((name.to_string(), roles, database));
        Ok([0x42u8; 16])
    }
    async fn drop_user(&self, name: &str) -> Result<bool, PortError> {
        self.drop_calls.lock().unwrap().push(name.to_string());
        Ok(false)
    }
    async fn grant_role(&self, _user: &str, _role: &str) -> Result<(), PortError> {
        Ok(())
    }
    async fn revoke_role(&self, _user: &str, _role: &str) -> Result<(), PortError> {
        Ok(())
    }
    async fn set_superuser(&self, _user: &str, _on: bool) -> Result<(), PortError> {
        Ok(())
    }
}

/// A real database owner (non-superuser `Actor::User`) creates a user
/// scoped to their own database end-to-end: `authorize_user_lifecycle`'s
/// Path 2 (database-owner delegation) admits, and the call actually reaches
/// the installed `UserAdminPort` with the right name/scope — proving the
/// task #559 seam wires delegation all the way through, not just at the
/// authorization-gate boundary already covered above.
#[tokio::test]
async fn db_owner_creates_scoped_user_through_port() {
    let shamir = setup().await;
    let owner = Actor::User(1001);
    make_db_owner(&shamir, owner.clone()).await;

    let port = Arc::new(RecordingPort::default());
    let shamir = shamir.with_user_admin_port(port.clone() as Arc<dyn UserAdminPort>);

    let req = one_op(ddl::create_user("scoped_erin", "pw").database("testdb"));
    let resp = shamir.execute_as(owner.clone(), "testdb", &req).await;
    assert!(
        resp.is_ok(),
        "a non-superuser database owner must create a user scoped to their own db: {resp:?}"
    );

    let calls = port.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "the port must be called exactly once");
    assert_eq!(calls[0].0, "scoped_erin");
    assert_eq!(
        calls[0].2.as_deref(),
        Some("testdb"),
        "the port must receive the owner's own database scope"
    );
}

// ===========================================================================
// C. Error classification — `handle_grant_role`/`handle_revoke_role`
// classify a `UserAdminPort` error containing "user not found" into code
// `not_found`, and any other message into the generic `query` (task #559
// review follow-up, #565).
// ===========================================================================

/// A `UserAdminPort` whose `grant_role`/`revoke_role` always fail with a
/// caller-supplied message. `create_user`/`drop_user`/`set_superuser` are
/// unused by these tests and just succeed trivially.
struct FailingRolePort {
    message: String,
}

#[async_trait]
impl UserAdminPort for FailingRolePort {
    async fn create_user(
        &self,
        _name: &str,
        _password: &str,
        _roles: Vec<String>,
        _database: Option<String>,
    ) -> Result<[u8; 16], PortError> {
        Ok([0u8; 16])
    }
    async fn drop_user(&self, _name: &str) -> Result<bool, PortError> {
        Ok(false)
    }
    async fn grant_role(&self, _user: &str, _role: &str) -> Result<(), PortError> {
        Err(self.message.clone().into())
    }
    async fn revoke_role(&self, _user: &str, _role: &str) -> Result<(), PortError> {
        Err(self.message.clone().into())
    }
    async fn set_superuser(&self, _user: &str, _on: bool) -> Result<(), PortError> {
        Ok(())
    }
}

#[tokio::test]
async fn grant_role_user_not_found_maps_to_not_found_code() {
    let shamir = setup().await;
    let port = Arc::new(FailingRolePort {
        message: "user not found: ghost".to_string(),
    });
    let shamir = shamir.with_user_admin_port(port as Arc<dyn UserAdminPort>);

    let req = one_op(ddl::grant_role("analyst", "ghost"));
    let resp = shamir.execute("testdb", &req).await;
    assert!(resp.is_err(), "expected an error: {resp:?}");
    assert_eq!(resp.unwrap_err().code(), Some("not_found"));
}

#[tokio::test]
async fn grant_role_other_error_maps_to_query_code() {
    let shamir = setup().await;
    let port = Arc::new(FailingRolePort {
        message: "directory I/O failure".to_string(),
    });
    let shamir = shamir.with_user_admin_port(port as Arc<dyn UserAdminPort>);

    let req = one_op(ddl::grant_role("analyst", "bob"));
    let resp = shamir.execute("testdb", &req).await;
    assert!(resp.is_err(), "expected an error: {resp:?}");
    assert_eq!(resp.unwrap_err().code(), Some("query"));
}

#[tokio::test]
async fn revoke_role_user_not_found_maps_to_not_found_code() {
    let shamir = setup().await;
    let port = Arc::new(FailingRolePort {
        message: "user not found: ghost".to_string(),
    });
    let shamir = shamir.with_user_admin_port(port as Arc<dyn UserAdminPort>);

    let req = one_op(ddl::revoke_role("analyst", "ghost"));
    let resp = shamir.execute("testdb", &req).await;
    assert!(resp.is_err(), "expected an error: {resp:?}");
    assert_eq!(resp.unwrap_err().code(), Some("not_found"));
}

#[tokio::test]
async fn revoke_role_other_error_maps_to_query_code() {
    let shamir = setup().await;
    let port = Arc::new(FailingRolePort {
        message: "directory I/O failure".to_string(),
    });
    let shamir = shamir.with_user_admin_port(port as Arc<dyn UserAdminPort>);

    let req = one_op(ddl::revoke_role("analyst", "bob"));
    let resp = shamir.execute("testdb", &req).await;
    assert!(resp.is_err(), "expected an error: {resp:?}");
    assert_eq!(resp.unwrap_err().code(), Some("query"));
}

// ===========================================================================
// D. No-resolver degrade — `handle_drop_user`'s scope lookup degrades to
// "global-admin only" when no `PrincipalResolver` is installed, even for
// the actual owner of the target user's database (documented
// safe-but-degraded behaviour, design doc §3.1; task #565).
// ===========================================================================

/// A database owner (non-superuser `Actor::User`) must NOT be able to drop
/// a user when no `PrincipalResolver` is installed: `drop_user`'s scope
/// lookup (`self.shamir.principal_resolver().and_then(...)`) has nothing to
/// resolve against, so it degrades to `None` — and `authorize_user_lifecycle`
/// only admits a global admin for an unscoped (`None`) target, denying the
/// owner. The port must never even be reached.
#[tokio::test]
async fn drop_user_without_resolver_degrades_to_global_admin_only() {
    let shamir = setup().await;
    let owner = Actor::User(1001);
    make_db_owner(&shamir, owner.clone()).await;

    // A UserAdminPort IS installed (storage is reachable in principle), but
    // NO PrincipalResolver — proving the degrade is specifically about scope
    // resolution, not a missing port.
    let port = Arc::new(RecordingPort::default());
    let shamir = shamir.with_user_admin_port(port.clone() as Arc<dyn UserAdminPort>);

    let req = one_op(ddl::drop_user("scoped_user"));
    let resp = shamir.execute_as(owner.clone(), "testdb", &req).await;
    assert!(
        resp.is_err(),
        "without a resolver, a db owner must NOT drop a user (scope degrades to None): {resp:?}"
    );
    assert!(
        port.drop_calls.lock().unwrap().is_empty(),
        "the port must not be reached — the authorization gate must deny first"
    );
}
