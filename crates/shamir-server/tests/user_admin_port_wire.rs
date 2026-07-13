//! Wire-level integration tests for task #559 (Store B retirement +
//! PrincipalResolver / UserAdminPort seam).
//!
//! Covers (per brief "Red tests required first"):
//!   1. create_user/drop_user/grant_role/revoke_role route through the port
//!      and actually persist to the REAL directory.
//!   3. CreateRole no longer parses as a BatchOp.
//!   4. access_tree/ListOp::Users reflect the resolver's data when a
//!      resolver is installed.
//!   5. The boot audit logs but never mutates.
//!   6. Database-scope owner-delegation still works end-to-end through the
//!      new resolver-backed scope lookup.
//!
//! Test #2 (not_supported without port) lives in `delegation_e2e.rs`.

use std::sync::{Arc, Mutex};

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_connect::server::user_record::UserRecord;
use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_types::batch::{BatchOp, BatchRequest, BatchResponse};
use shamir_query_types::hmac as canon;
use shamir_query_types::wire::DbResponse;
use shamir_server::db_handler::{AdminGlue, ShamirDbHandler};
use shamir_server::ports::DirectoryPorts;
use shamir_server::server::audit_store_b_vs_directory;
use shamir_server::user_directory::FjallUserDirectory;
use shamir_types::codecs::interned::query_value_to_inner;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;
use tempfile::TempDir;
use tracing::Subscriber;
use tracing_subscriber::fmt::MakeWriter;
use zeroize::Zeroizing;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn fixture_record() -> UserRecord {
    let salt = [0xa1u8; 16];
    let stored = StoredKey([0xc3u8; 32]);
    let mut server_key = Zeroizing::new([0u8; 32]);
    for (i, b) in server_key.iter_mut().enumerate() {
        *b = i as u8;
    }
    UserRecord {
        salt,
        stored_key: stored,
        server_key,
        kdf_params: KdfParams::DEFAULT,
        tickets_invalid_before_ns: 0,
    }
}

fn root_session() -> Session {
    Session::new(
        [0xAB; 16],
        "root".into(),
        SessionPermissions::from_roles(vec!["superuser".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

fn session_key(session: &Session) -> [u8; 32] {
    canon::derive_session_hmac_key(&session.session_id)
}

fn expect_error(res: DbResponse) -> (String, String) {
    match res {
        DbResponse::Error { code, message } => (code, message),
        other => panic!("expected Error, got {:?}", other),
    }
}

/// Build a handler wired with AdminGlue AND DirectoryPorts over a fresh
/// FjallUserDirectory + in-memory ShamirDb. This is the key difference from
/// `set_superuser_wire.rs`'s `build_handler` — the ports are injected so
/// the BatchOp-level user-admin handlers route through the real directory.
async fn build_handler_with_ports() -> (ShamirDbHandler, Arc<FjallUserDirectory>, Arc<ShamirDb>) {
    let tmp = TempDir::new().unwrap();
    let user_dir = Arc::new(FjallUserDirectory::open(tmp.path().join("u.redb")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    // Create the "scratch" database so batch dispatch can target it.
    db.create_db("scratch").await;
    // Inject the identity-seam ports BEFORE wrapping in Arc.
    let (port, resolver) = DirectoryPorts::new(user_dir.clone(), fast_kdf()).into_trait_objects();
    let db = db
        .with_user_admin_port(port)
        .with_principal_resolver(resolver);
    let db_arc = Arc::new(db);
    let handler = ShamirDbHandler::with_admin(
        db_arc.clone(),
        AdminGlue {
            user_dir: user_dir.clone(),
            kdf: fast_kdf(),
            tables_registry: None,
        },
    );
    std::mem::forget(tmp);
    (handler, user_dir, db_arc)
}

/// Build a handler WITHOUT ports (for testing absent-resolver behavior).
async fn build_handler_without_ports() -> ShamirDbHandler {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    db.create_db("scratch").await;
    ShamirDbHandler::new(Arc::new(db))
}

/// Execute a batch request through the handler (the same dispatch path a
/// real connection uses). Returns the decoded DbResponse — for success
/// it's `DbResponse::Batch`, for failure it's `DbResponse::Error`.
async fn exec_batch_raw(
    handler: &ShamirDbHandler,
    session: &Session,
    req: &BatchRequest,
) -> DbResponse {
    // Wrap the BatchRequest in a DbRequest::Execute envelope — the handler
    // expects the full wire envelope, not a bare BatchRequest.
    let wire_req = shamir_query_types::wire::DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "scratch".to_string(),
        batch: req.clone(),
    };
    let req_bytes = rmp_serde::to_vec_named(&wire_req).expect("encode request");
    let res = handler
        .handle(session, &req_bytes, &ConnectionServices::without_push(0))
        .await
        .unwrap();
    rmp_serde::from_slice(&res).expect("decode response")
}

/// Execute a batch and extract the BatchResponse (panics if it's an Error).
async fn exec_batch(
    handler: &ShamirDbHandler,
    session: &Session,
    req: &BatchRequest,
) -> BatchResponse {
    match exec_batch_raw(handler, session, req).await {
        DbResponse::Batch { response } => response,
        DbResponse::Error { code, message } => {
            panic!("batch failed: code={code}, message={message}")
        }
        other => panic!("expected Batch, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Test 1: create_user/drop_user/grant_role/revoke_role route through the
// port and actually persist to the REAL directory.
// ---------------------------------------------------------------------------

/// create_user through the BatchOp path persists to the directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_user_routes_through_port_to_real_directory() {
    let (handler, user_dir, _db) = build_handler_with_ports().await;

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_create_user("alice"));

    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.create_user(
        "u",
        shamir_query_builder::ddl::create_user("alice", "s3cretpw").hmac(&tag),
    );
    let req = b.to_request_via_msgpack();
    let resp = exec_batch(&handler, &session, &req).await;
    assert!(
        resp.results.contains_key("u"),
        "batch should have result 'u'"
    );

    // The REAL directory must reflect the change — this is the key
    // assertion proving Store B was bypassed.
    let uid = user_dir.user_id("alice");
    assert!(uid.is_some(), "alice must exist in the real directory");
    let state = user_dir.state_by_user_id(&uid.unwrap()).unwrap();
    assert_eq!(state.username, "alice");
}

/// drop_user through the BatchOp path removes from the real directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_user_routes_through_port_and_removes_from_directory() {
    let (handler, user_dir, _db) = build_handler_with_ports().await;

    // Seed a user directly in the directory.
    let uid = user_dir
        .insert("bob".to_string(), fixture_record())
        .unwrap();

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_drop_user("bob"));

    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.drop_user("d", shamir_query_builder::ddl::drop_user("bob").hmac(&tag));
    let req = b.to_request_via_msgpack();
    let resp = exec_batch(&handler, &session, &req).await;
    assert!(resp.results.contains_key("d"));

    // The directory must reflect the removal.
    assert!(
        user_dir.state_by_user_id(&uid).is_none(),
        "bob must be removed from the real directory"
    );
}

/// grant_role through the BatchOp path persists to the real directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_role_routes_through_port_to_real_directory() {
    let (handler, user_dir, _db) = build_handler_with_ports().await;

    // Seed a user directly in the directory.
    let uid = user_dir
        .insert("carol".to_string(), fixture_record())
        .unwrap();

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_grant_role("analyst", "carol"));

    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.grant_role(
        "g",
        shamir_query_builder::ddl::grant_role("analyst", "carol").hmac(&tag),
    );
    let req = b.to_request_via_msgpack();
    let resp = exec_batch(&handler, &session, &req).await;
    assert!(resp.results.contains_key("g"));

    // The directory must reflect the granted role.
    let state = user_dir.state_by_user_id(&uid).unwrap();
    assert!(
        state.roles.iter().any(|r| r == "analyst"),
        "carol must have 'analyst' role in the directory after grant: {:?}",
        state.roles
    );
}

/// revoke_role through the BatchOp path persists to the real directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_role_routes_through_port_to_real_directory() {
    let (handler, user_dir, _db) = build_handler_with_ports().await;

    // Seed a user with a role directly in the directory.
    let uid = user_dir
        .insert("dave".to_string(), fixture_record())
        .unwrap();
    user_dir
        .update_roles("dave", vec!["analyst".to_string()], 0)
        .unwrap();

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_revoke_role("analyst", "dave"));

    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.revoke_role(
        "r",
        shamir_query_builder::ddl::revoke_role("analyst", "dave").hmac(&tag),
    );
    let req = b.to_request_via_msgpack();
    let resp = exec_batch(&handler, &session, &req).await;
    assert!(resp.results.contains_key("r"));

    // The directory must reflect the revoked role.
    let state = user_dir.state_by_user_id(&uid).unwrap();
    assert!(
        !state.roles.iter().any(|r| r == "analyst"),
        "dave must NOT have 'analyst' role after revoke: {:?}",
        state.roles
    );
}

// ---------------------------------------------------------------------------
// Test 3: CreateRole no longer parses as a BatchOp (deleted variant).
// ---------------------------------------------------------------------------

/// A wire request with `create_role` key fails to parse as a BatchOp
/// (unknown variant after deletion).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_role_wire_request_fails_to_parse() {
    // Construct a raw msgpack map with a "create_role" key — this used to
    // parse as BatchOp::CreateRole but should now fail.
    let raw = rmp_serde::to_vec_named(&shamir_types::types::value::QueryValue::Map({
        let mut m = shamir_types::types::common::new_map();
        m.insert(
            "create_role".to_string(),
            shamir_types::types::value::QueryValue::Str("viewer".to_string()),
        );
        m.insert(
            "permissions".to_string(),
            shamir_types::types::value::QueryValue::List(Vec::new()),
        );
        m
    }))
    .unwrap();

    let result: Result<BatchOp, _> = rmp_serde::from_slice(&raw);
    assert!(
        result.is_err(),
        "create_role must no longer parse as a BatchOp"
    );
}

// ---------------------------------------------------------------------------
// Test 4: ListOp::Users reflects the resolver's data when installed.
// ---------------------------------------------------------------------------

/// ListOp::Users reflects the resolver's directory data (not Store B).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_users_reflects_resolver_data() {
    let (handler, user_dir, _db) = build_handler_with_ports().await;

    // Seed two users in the directory.
    user_dir
        .insert("alice".to_string(), fixture_record())
        .unwrap();
    user_dir
        .insert("bob".to_string(), fixture_record())
        .unwrap();

    let session = root_session();
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.list_users("list", shamir_query_builder::ddl::list_users());
    let req = b.to_request_via_msgpack();
    let resp = exec_batch(&handler, &session, &req).await;

    let rec = resp.results["list"]
        .records
        .first()
        .expect("list result has a record");
    let val = rec.as_value();
    let users = val["users"].as_array().expect("users field is an array");
    let names: Vec<&str> = users
        .iter()
        .filter_map(|u| u.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(
        names.contains(&"alice"),
        "alice must be listed: {:?}",
        names
    );
    assert!(names.contains(&"bob"), "bob must be listed: {:?}", names);
}

/// ListOp::Users without a resolver returns not_supported (batch-level error).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_users_without_resolver_returns_not_supported() {
    let handler = build_handler_without_ports().await;

    let session = root_session();
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.list_users("list", shamir_query_builder::ddl::list_users());
    let req = b.to_request_via_msgpack();
    let res = exec_batch_raw(&handler, &session, &req).await;
    let (code, _msg) = expect_error(res);
    assert_eq!(
        code, "not_supported",
        "list_users without resolver should return not_supported"
    );
}

// ---------------------------------------------------------------------------
// Test 5: The boot audit logs but never mutates.
// ---------------------------------------------------------------------------

/// `CaptureWriter` that appends every byte written by the tracing layer
/// into a shared `Vec<u8>` so the test can assert on the captured text.
/// Mirrors the pattern in `slow_query_log.rs`.
#[derive(Clone)]
struct CaptureWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}
struct CaptureGuard {
    buf: Arc<Mutex<Vec<u8>>>,
}
impl std::io::Write for CaptureGuard {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buf.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureGuard;
    fn make_writer(&'a self) -> Self::Writer {
        CaptureGuard {
            buf: self.buf.clone(),
        }
    }
}

/// The boot audit runs without error, doesn't mutate either store, and
/// emits no spurious WARNs for a clean deployment (directory canonical,
/// Store B empty).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_audit_runs_without_mutating_stores() {
    let tmp = TempDir::new().unwrap();

    // 1. Wire a tracing subscriber whose writer captures into a Vec.
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = CaptureWriter { buf: buf.clone() };
    let subscriber: Box<dyn Subscriber + Send + Sync> = Box::new(
        tracing_subscriber::fmt()
            .with_writer(writer)
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish(),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    // 2. Build ShamirDb + directory.
    let meta_path = tmp.path().join("meta.redb");
    let shamir = ShamirDb::init(SystemStoreConfig::Fjall(meta_path))
        .await
        .unwrap();
    let user_dir = FjallUserDirectory::open(tmp.path().join("users")).unwrap();

    // 3. Seed the directory with a user (Store B stays empty — this is the
    //    normal post-#559 state: the directory is canonical, Store B is
    //    retired and empty).
    user_dir
        .insert("real_user".to_string(), fixture_record())
        .unwrap();

    // 4. Run the audit.
    audit_store_b_vs_directory(&shamir, &user_dir).await;

    // 5. Assert no WARN was emitted (Store B is empty, nothing is divergent;
    //    the directory-has-user-not-in-Store-B case is debug-level only).
    let captured = {
        let lock = buf.lock().unwrap();
        String::from_utf8_lossy(&lock).into_owned()
    };
    assert!(
        !captured.contains("WARN") || captured.is_empty(),
        "audit should not emit WARN for a clean state: captured: {}",
        captured
    );

    // 6. Assert NEITHER store was mutated.
    let store_b = shamir.system_store().load_users().await.unwrap();
    assert!(
        store_b.is_empty(),
        "Store B must still be empty (audit is read-only)"
    );
    assert!(
        user_dir.user_id("real_user").is_some(),
        "directory must still have real_user (audit is read-only)"
    );

    std::mem::forget(tmp);
}

/// Seed a raw record directly into shamir-db's retired Store B `users`
/// table — bypasses the (now-removed) `BatchOp::CreateUser` write path,
/// which is the whole point: this reproduces the pre-#559 divergent-state
/// scenarios the boot audit (`audit_store_b_vs_directory`) must warn about,
/// none of which are reachable through any live write path anymore.
async fn seed_store_b_user(shamir: &ShamirDb, name: &str, roles: &[&str]) {
    let table = shamir.system_store().users_table().await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let qv = mpack!({
        "name": @(QueryValue::Str(name.to_string())),
        "roles": @(QueryValue::List(
            roles.iter().map(|r| QueryValue::Str(r.to_string())).collect(),
        )),
    });
    let inner = query_value_to_inner(&qv, interner).unwrap();
    table.insert(&inner).await.unwrap();
}

/// A username present in Store B but NOT in the directory (phantom — it
/// never had a live login) triggers a WARN naming the user.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_audit_warns_on_phantom_store_b_user() {
    let tmp = TempDir::new().unwrap();
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = CaptureWriter { buf: buf.clone() };
    let subscriber: Box<dyn Subscriber + Send + Sync> = Box::new(
        tracing_subscriber::fmt()
            .with_writer(writer)
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish(),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let meta_path = tmp.path().join("meta.redb");
    let shamir = ShamirDb::init(SystemStoreConfig::Fjall(meta_path))
        .await
        .unwrap();
    let user_dir = FjallUserDirectory::open(tmp.path().join("users")).unwrap();

    // "ghost" exists only in Store B — the directory is empty.
    seed_store_b_user(&shamir, "ghost", &["analyst"]).await;

    audit_store_b_vs_directory(&shamir, &user_dir).await;

    let captured = {
        let lock = buf.lock().unwrap();
        String::from_utf8_lossy(&lock).into_owned()
    };
    assert!(
        captured.contains("WARN") && captured.contains("ghost"),
        "expected a WARN naming the phantom Store B user 'ghost': captured: {}",
        captured
    );
    assert!(
        captured.contains("NOT in the real directory"),
        "expected the phantom-user WARN message: captured: {}",
        captured
    );

    std::mem::forget(tmp);
}

/// A user present in BOTH stores with DIVERGENT role sets triggers a WARN.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_audit_warns_on_role_divergence() {
    let tmp = TempDir::new().unwrap();
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = CaptureWriter { buf: buf.clone() };
    let subscriber: Box<dyn Subscriber + Send + Sync> = Box::new(
        tracing_subscriber::fmt()
            .with_writer(writer)
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish(),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let meta_path = tmp.path().join("meta.redb");
    let shamir = ShamirDb::init(SystemStoreConfig::Fjall(meta_path))
        .await
        .unwrap();
    let user_dir = FjallUserDirectory::open(tmp.path().join("users")).unwrap();

    // "carol" exists in BOTH stores, but Store B's role list ("editor")
    // diverges from the directory's ("analyst").
    user_dir
        .insert("carol".to_string(), fixture_record())
        .unwrap();
    user_dir.grant_role("carol", "analyst", 0).unwrap();
    seed_store_b_user(&shamir, "carol", &["editor"]).await;

    audit_store_b_vs_directory(&shamir, &user_dir).await;

    let captured = {
        let lock = buf.lock().unwrap();
        String::from_utf8_lossy(&lock).into_owned()
    };
    assert!(
        captured.contains("WARN") && captured.contains("carol"),
        "expected a WARN naming the divergent user 'carol': captured: {}",
        captured
    );
    assert!(
        captured.contains("role sets diverge"),
        "expected the role-divergence WARN message: captured: {}",
        captured
    );

    std::mem::forget(tmp);
}

/// Store B has a phantom "superuser" grant the directory does NOT have —
/// triggers the dedicated phantom-superuser WARN.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_audit_warns_on_phantom_superuser() {
    let tmp = TempDir::new().unwrap();
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = CaptureWriter { buf: buf.clone() };
    let subscriber: Box<dyn Subscriber + Send + Sync> = Box::new(
        tracing_subscriber::fmt()
            .with_writer(writer)
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish(),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let meta_path = tmp.path().join("meta.redb");
    let shamir = ShamirDb::init(SystemStoreConfig::Fjall(meta_path))
        .await
        .unwrap();
    let user_dir = FjallUserDirectory::open(tmp.path().join("users")).unwrap();

    // "dave" exists in both stores; Store B claims a "superuser" grant the
    // directory does not have (directory superuser stays false/default).
    user_dir
        .insert("dave".to_string(), fixture_record())
        .unwrap();
    seed_store_b_user(&shamir, "dave", &["superuser"]).await;

    audit_store_b_vs_directory(&shamir, &user_dir).await;

    let captured = {
        let lock = buf.lock().unwrap();
        String::from_utf8_lossy(&lock).into_owned()
    };
    assert!(
        captured.contains("WARN") && captured.contains("dave"),
        "expected a WARN naming the phantom-superuser user 'dave': captured: {}",
        captured
    );
    assert!(
        captured.contains("phantom superuser"),
        "expected the phantom-superuser WARN message: captured: {}",
        captured
    );

    std::mem::forget(tmp);
}

// ---------------------------------------------------------------------------
// Test 6: Database-scope owner-delegation works end-to-end through the
// resolver-backed scope lookup.
// ---------------------------------------------------------------------------

/// A scoped user created via the port has its `database` field set in the
/// directory record, so `authorize_user_lifecycle`'s scope lookup via the
/// resolver correctly resolves the scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_user_has_database_field_in_directory() {
    let (handler, user_dir, _db) = build_handler_with_ports().await;

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_create_user("scoped_erin"));

    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.create_user(
        "u",
        shamir_query_builder::ddl::create_user("scoped_erin", "pw")
            .database("mydb")
            .hmac(&tag),
    );
    let req = b.to_request_via_msgpack();
    let resp = exec_batch(&handler, &session, &req).await;
    assert!(resp.results.contains_key("u"));

    // The directory record must carry the `database` scope.
    let uid = user_dir.user_id("scoped_erin").expect("user exists");
    let state = user_dir.state_by_user_id(&uid).unwrap();
    assert_eq!(
        state.database.as_deref(),
        Some("mydb"),
        "scoped user must have database='mydb' in the directory record"
    );
}

// NOTE: a wire-level positive test for database-owner delegation was
// attempted here and removed — it is architecturally unreachable. The
// coarse admin/auth gate added by task #553 (`tx_handlers.rs`/`handler.rs`:
// "query '{alias}' requires superuser (admin/auth op)") rejects ANY admin
// op from a non-superuser session before it ever reaches
// `authorize_user_lifecycle`'s database-owner path. That gate predates
// #559 and reworking it is out of scope here. The genuine end-to-end
// delegation + port-routing proof lives at the shamir-db level (which
// calls `execute_as` directly, bypassing the wire's coarse gate) in
// `crates/shamir-db/tests/delegation_e2e.rs::db_owner_creates_scoped_user_through_port`.

// ---------------------------------------------------------------------------
// Regression: create_user with the reserved "superuser" role must fail
// atomically — no orphan, roleless account left behind in the directory.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_user_with_reserved_superuser_role_leaves_no_orphan_account() {
    let (handler, user_dir, _db) = build_handler_with_ports().await;

    let session = root_session();
    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_create_user("frank"));

    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.create_user(
        "u",
        shamir_query_builder::ddl::create_user("frank", "pw")
            .roles(vec!["superuser".to_string()])
            .hmac(&tag),
    );
    let req = b.to_request_via_msgpack();
    let res = exec_batch_raw(&handler, &session, &req).await;
    expect_error(res);

    assert!(
        user_dir.user_id("frank").is_none(),
        "frank must NOT be persisted in the directory after create_user \
         is rejected for requesting the reserved 'superuser' role"
    );
}
