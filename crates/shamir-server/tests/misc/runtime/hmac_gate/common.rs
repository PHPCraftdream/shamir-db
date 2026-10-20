use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_query_types::hmac as canon;
use shamir_server::db_handler::{AdminGlue, DbRequest, DbResponse, ShamirDbHandler};
use shamir_server::user_directory::FjallUserDirectory;
use std::sync::Arc;
use tempfile::TempDir;

// --------------------------------------------------------------------------
// Fixtures
// --------------------------------------------------------------------------

/// Build a superuser session. Bypassing the SessionStore means
/// `session_id` stays at zeros, which is fine — what matters for
/// HMAC validation is that the test computes its tag with the
/// SAME `session_id` the server sees.
pub(crate) fn root_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["superuser".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

pub(crate) fn session_key(session: &Session) -> [u8; 32] {
    canon::derive_session_hmac_key(&session.session_id)
}

pub(crate) async fn make_db_with_table(db: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.expect("add repo");
    Arc::new(shamir)
}

pub(crate) fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

pub(crate) fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}

pub(crate) fn execute_built(db: &str, batch: BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch,
    }
}

pub(crate) fn expect_error(res: DbResponse) -> (String, String) {
    match res {
        DbResponse::Error { code, message } => (code, message),
        other => panic!("expected Error, got {:?}", other),
    }
}

pub(crate) fn expect_batch_ok(res: DbResponse) -> shamir_db::query::batch::BatchResponse {
    match res {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch, got {:?}", other),
    }
}

pub(crate) fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

/// Build a handler wired with `AdminGlue` (real `FjallUserDirectory`) — the
/// `DbRequest::CreateScramUser` handler needs this to run past the HMAC
/// gate and actually create the user (mirrors `set_superuser_wire.rs`'s
/// `build_handler`).
pub(crate) async fn build_handler_with_admin() -> ShamirDbHandler {
    let tmp = TempDir::new().unwrap();
    let user_dir = Arc::new(FjallUserDirectory::open(tmp.path().join("u.redb")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::with_admin(
        Arc::new(db),
        AdminGlue {
            user_dir,
            kdf: fast_kdf(),
            tables_registry: None,
        },
    );
    // Hold `tmp` alive for the test's lifetime by leaking it — mirrors
    // `set_superuser_wire.rs`'s fixture.
    std::mem::forget(tmp);
    handler
}
