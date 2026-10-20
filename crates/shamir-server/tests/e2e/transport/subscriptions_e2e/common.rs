//! Shared fixtures for the subscriptions e2e tests: push capture, sessions,
//! in-memory dbs, request codecs and push-frame helpers.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::timeout;

use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::{ConnectionServices, PushRejected, PushSink};
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_types::batch::BatchRequest;

use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::subscriptions::SubscriptionRegistry;

// ---------- capture-push: drains frames into an mpsc channel ----------

struct CapturePush {
    tx: UnboundedSender<Vec<u8>>,
}

impl PushSink for CapturePush {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        // If the receiver is dropped the test is over — ignore.
        let _ = self.tx.send(frame);
        Ok(())
    }
}

pub(crate) fn capture() -> (Arc<dyn PushSink>, UnboundedReceiver<Vec<u8>>) {
    let (tx, rx) = unbounded_channel();
    (Arc::new(CapturePush { tx }), rx)
}

// ---------- session / db fixtures ----------

pub(crate) fn user_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    s.session_id = [0x11; 32];
    s
}

pub(crate) async fn make_db_one_repo(db: &str, repo: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.expect("add repo");
    // G.4c: new objects default to enforced (0o700, System). These tests run
    // subscribe/insert ops as a non-superuser user session (Actor::User), so
    // open the db + repo + table so the user can traverse and read/write.
    // The tests are about subscription semantics, not access control.
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

pub(crate) async fn make_db_two_repos(
    db: &str,
    repo_a: &str,
    table_a: &str,
    repo_b: &str,
    table_b: &str,
) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg_a =
        RepoConfig::new(repo_a, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table_a));
    shamir.add_repo(db, cfg_a).await.expect("add repo a");
    let cfg_b =
        RepoConfig::new(repo_b, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table_b));
    shamir.add_repo(db, cfg_b).await.expect("add repo b");
    // G.4c: open the db + both repos + both tables so the non-superuser user
    // session can traverse them (these tests are about subscription semantics).
    let open = shamir_types::access::ResourceMeta::open();
    shamir
        .set_resource_meta(&shamir_types::access::ResourcePath::database(db), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &shamir_types::access::ResourcePath::store(db, repo_a),
            &open,
        )
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &shamir_types::access::ResourcePath::store(db, repo_b),
            &open,
        )
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &shamir_types::access::ResourcePath::table(db, repo_a, table_a),
            &open,
        )
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &shamir_types::access::ResourcePath::table(db, repo_b, table_b),
            &open,
        )
        .await
        .unwrap();
    Arc::new(shamir)
}

// ---------- request encoding helpers ----------

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

pub(crate) fn conn_with(
    push: Arc<dyn PushSink>,
    registry: Arc<SubscriptionRegistry>,
) -> ConnectionServices {
    ConnectionServices {
        conn_id: 1,
        push: Some(push),
        extensions: Some(registry as Arc<dyn std::any::Any + Send + Sync>),
    }
}

/// Decode a `BatchResponse` from a successful `DbResponse::Batch` or panic.
pub(crate) fn unwrap_batch(resp: DbResponse) -> shamir_query_types::batch::BatchResponse {
    match resp {
        DbResponse::Batch { response } => response,
        other => panic!("expected DbResponse::Batch, got {:?}", other),
    }
}

/// Pull the `sub` id the server injected into `results[alias].value.sub`.
pub(crate) fn sub_id_of(resp: &shamir_query_types::batch::BatchResponse, alias: &str) -> u64 {
    use shamir_types::types::value::QueryValue;
    let qr = resp
        .results
        .get(alias)
        .unwrap_or_else(|| panic!("missing result for alias {alias}"));
    let v = qr.value.as_ref().expect("subscribe result has no `value`");
    match v {
        QueryValue::Map(m) => match m.get("sub") {
            Some(QueryValue::Int(id)) => *id as u64,
            other => panic!("subscribe result `sub` is not Int: {:?}", other),
        },
        other => panic!("subscribe result value is not Map: {:?}", other),
    }
}

/// Wait up to `dur` for a single push frame.
pub(crate) async fn next_push(
    rx: &mut UnboundedReceiver<Vec<u8>>,
    dur: Duration,
) -> Option<PushEnvelope> {
    let frame = timeout(dur, rx.recv()).await.ok().flatten()?;
    Some(rmp_serde::from_slice(&frame).expect("decode push envelope"))
}

/// Wait for the first `Event` frame for `sub_id`, draining other kinds
/// (`Ready`, `Gap`, `Closed`, `SlowConsumer`) up to `dur` total.
pub(crate) async fn next_event_for(
    rx: &mut UnboundedReceiver<Vec<u8>>,
    sub_id: u64,
    dur: Duration,
) -> Option<PushEnvelope> {
    let deadline = tokio::time::Instant::now() + dur;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return None;
        }
        let remaining = deadline - now;
        let env = next_push(rx, remaining).await?;
        if env.sub == sub_id && env.push == PushKind::Event {
            return Some(env);
        }
        // Otherwise keep draining.
    }
}

/// Bridge tasks subscribe to the changefeed asynchronously after the
/// subscribe response is sent. Give them a moment to attach before the
/// next write batch fires events.
pub(crate) async fn settle() {
    tokio::time::sleep(Duration::from_millis(80)).await;
}
