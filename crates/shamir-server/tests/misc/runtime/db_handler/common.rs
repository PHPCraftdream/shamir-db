//! Shared fixtures for the `db_handler` integration tests: sessions,
//! in-memory db setup and request codecs.

use std::sync::Arc;

use indexmap::IndexMap;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

use shamir_server::db_handler::{DbRequest, DbResponse};

pub(crate) fn make_session(roles: Vec<String>) -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(roles),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

pub(crate) fn user_session() -> Session {
    make_session(vec!["read_write".into()])
}

pub(crate) fn root_session() -> Session {
    make_session(vec!["superuser".into()])
}

pub(crate) async fn make_db_with_table(db: &str, repo: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await.expect("create_db");
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.expect("add repo");
    // G.4c: new objects default to enforced (0o700, System). These tests run
    // DML as a non-superuser user session (Actor::User), so open the db +
    // repo + table so the user can traverse and read/write. The tests are
    // not about access control — they exercise wire semantics, limits,
    // pagination, etc.
    let open = shamir_types::access::ResourceMeta::open();
    shamir
        .set_resource_meta(&shamir_types::access::ResourcePath::database(db), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&shamir_types::access::ResourcePath::store(db, repo), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &shamir_types::access::ResourcePath::table(db, repo, table),
            &open,
        )
        .await
        .unwrap();
    Arc::new(shamir)
}

pub(crate) fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

pub(crate) fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}

/// Build a `DbRequest::Execute` from a pre-built [`BatchRequest`].
pub(crate) fn execute_built(db: &str, batch: BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch,
    }
}

/// Same as above but with an explicit `query_version`.
pub(crate) fn execute_built_with_version(
    db: &str,
    query_version: u32,
    batch: BatchRequest,
) -> DbRequest {
    DbRequest::Execute {
        query_version,
        db: db.to_string(),
        batch,
    }
}

// --------------------------------------------------------------------------
// Sanity: `IndexMap` is in the import set so that future tests can build
// `TMap` directly without going through a serialised format if they need to.
// --------------------------------------------------------------------------
#[allow(dead_code)]
fn _indexmap_in_scope() -> IndexMap<String, u32> {
    IndexMap::new()
}
