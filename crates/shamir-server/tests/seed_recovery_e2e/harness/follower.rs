//! Follower-side drivers: in-process fresh followers, seeded homes opened
//! from disk, row/bookmark/status observations, and the supervisor wired to
//! the real wire factory (production `build_prod_factory`).

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use shamir_client::{Client, ConnectOptions};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_db::SystemStoreConfig;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::query::Query;
use shamir_query_types::batch::BatchError;
use shamir_query_types::wire::repl::ReplRequest;
use shamir_server::replication::prod_factory::build_prod_factory;
use shamir_server::replication::supervisor::SubscriptionSupervisor;
use shamir_types::types::value::QueryValue;
use zeroize::Zeroizing;

use super::child;
use super::wire;

/// A fresh, empty in-memory follower home with db app / repo main and the
/// two replicated tables already attached (mirrors
/// `recovery_gap_consumer_tests::open`).
pub async fn fresh_in_memory() -> ShamirDb {
    let follower = ShamirDb::init_memory().await.expect("follower init");
    follower.create_db(wire::DB).await.expect("create_db");
    let mut config = RepoConfig::new(wire::REPO, BoxRepoFactory::in_memory());
    for table in wire::TABLES {
        config = config.add_table(TableConfig::new(table));
    }
    follower
        .add_repo(wire::DB, config)
        .await
        .expect("follower repo");
    follower
}

/// Open a seeded/installed follower home from disk. `init_seed` runs the
/// boot gate: an abandoned (incomplete) installation is never registered.
pub async fn open_home(data_dir: &Path) -> ShamirDb {
    ShamirDb::init_seed(SystemStoreConfig::Fjall(
        data_dir.join("shamir_db_meta.redb"),
    ))
    .await
    .expect("open follower home")
}

/// True when an ordinary read of `table` is refused with the typed
/// `resync_required` quarantine error (S5c-X PR8b).
pub async fn read_refused_resync(home: &ShamirDb, table: &str) -> bool {
    let mut batch = Batch::new();
    batch.id("read");
    batch.query("rows", Query::with_repo(wire::REPO, table));
    matches!(
        home.execute(wire::DB, &batch.build()).await,
        Err(BatchError::ResyncRequired { .. })
    )
}

pub async fn rows(home: &ShamirDb, table: &str) -> Vec<QueryValue> {
    let mut batch = Batch::new();
    batch.id("read");
    batch.query("rows", Query::with_repo(wire::REPO, table));
    let response = home.execute(wire::DB, &batch.build()).await.expect("read");
    let result = &response.results["rows"];
    assert!(!result.skipped, "home read skipped: {result:?}");
    assert!(
        result.corrupt_records.is_empty(),
        "home read corrupt: {result:?}",
    );
    let mut rows: Vec<QueryValue> = result
        .records
        .iter()
        .map(|record| record.as_value().into_owned())
        .collect();
    wire::sort_rows(&mut rows);
    rows
}

pub async fn bookmark(home: &ShamirDb) -> u64 {
    home.get_db(wire::DB)
        .and_then(|db| db.get_repo(wire::REPO))
        .expect("home repo registered")
        .replication_bookmark()
        .await
        .expect("bookmark read")
}

/// The subscription's status entry (SEED-4 enriched record) as a map, or
/// `None` while the catalogue has no such subscription.
pub async fn status_of(home: &ShamirDb, name: &str) -> Option<QueryValue> {
    let mut batch = Batch::new();
    batch.id("status");
    batch.op("status", ddl::replication_status());
    let response = home
        .execute(wire::DB, &batch.build())
        .await
        .expect("status");
    let entry = &response.results["status"];
    let value = entry.records.first()?.as_value().into_owned();
    let QueryValue::List(subscriptions) = value.get("subscriptions")? else {
        panic!("status without subscription list: {value:?}");
    };
    subscriptions
        .iter()
        .find(|sub| sub.get("name").and_then(QueryValue::as_str) == Some(name))
        .cloned()
}

pub fn field<'a>(entry: &'a QueryValue, key: &str) -> &'a QueryValue {
    entry
        .get(key)
        .unwrap_or_else(|| panic!("status entry missing {key}: {entry:?}"))
}

pub fn str_field<'a>(entry: &'a QueryValue, key: &str) -> &'a str {
    field(entry, key)
        .as_str()
        .unwrap_or_else(|| panic!("status field {key} is not a string: {entry:?}"))
}

pub fn u64_field(entry: &QueryValue, key: &str) -> Option<u64> {
    match field(entry, key) {
        QueryValue::Null => None,
        value => Some(
            value
                .as_i64()
                .unwrap_or_else(|| panic!("status field {key} is not numeric: {value:?}"))
                as u64,
        ),
    }
}

/// The production supervisor over `home`, pulling from the leader child
/// through the real TLS+SCRAM wire factory. Constructing it also installs
/// the resume-validation probe on `home` (supervisor boot wiring), so
/// Resume validation uses the same authenticated Hello the loop uses.
pub fn supervisor(home: &Arc<ShamirDb>) -> SubscriptionSupervisor {
    let factory = build_prod_factory(
        Some(child::REPLICATOR.to_string()),
        Some(child::REPLICATOR_PW.to_string()),
        "localhost".to_string(),
    )
    .expect("replicator credentials resolve");
    SubscriptionSupervisor::new(home.clone(), factory, "seed-e2e-follower")
        .with_poll_wait_ms(200)
        .with_max_iterations(400)
        .with_validate_replicated_payload(false)
}

/// Bounded, deadline-based wait on an observable condition. Each poll is
/// individually bounded by the same absolute deadline; diagnostics run once
/// after timeout and have their own short bound.
pub async fn wait_until<F, Fut>(
    what: &str,
    deadline: Duration,
    home: &ShamirDb,
    leader_addr: SocketAddr,
    mut condition: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let end = tokio::time::Instant::now() + deadline;
    loop {
        match tokio::time::timeout_at(end, condition()).await {
            Ok(true) => return,
            Ok(false) => {}
            Err(_) => break,
        }
        let now = tokio::time::Instant::now();
        if now >= end {
            break;
        }
        tokio::time::sleep_until((now + Duration::from_millis(100)).min(end)).await;
        if tokio::time::Instant::now() >= end {
            break;
        }
    }

    let follower = async {
        let mut batch = Batch::new();
        batch.id("status");
        batch.op("status", ddl::replication_status());
        let status = home.execute(wire::DB, &batch.build()).await;
        let status = match status {
            Ok(response) => format!("Ok({:?})", response.results.get("status")),
            Err(error) => format!("Err({error:?})"),
        };
        let bookmark = match home.get_db(wire::DB).and_then(|db| db.get_repo(wire::REPO)) {
            Some(repo) => format!("{:?}", repo.replication_bookmark().await),
            None => "Err(repo not registered)".to_string(),
        };
        (status, bookmark)
    };
    let leader = async {
        match Client::connect(ConnectOptions {
            addr: leader_addr,
            server_name: "localhost".to_string(),
            username: child::REPLICATOR.to_string(),
            password: Zeroizing::new(child::REPLICATOR_PW.as_bytes().to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: Some(Duration::from_secs(15)),
            handshake_timeout: None,
            request_timeout: Some(Duration::from_secs(35)),
        })
        .await
        {
            Ok(client) => format!(
                "{:?}",
                client
                    .repl(ReplRequest::Hello {
                        proto_ver: 1,
                        node_id: "seed-e2e-timeout-diagnostic".to_string(),
                        capabilities: None,
                    })
                    .await
            ),
            Err(error) => format!("connect error: {error:?}"),
        }
    };
    let (follower_snapshot, leader_snapshot) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(3), follower),
        tokio::time::timeout(Duration::from_secs(3), leader),
    );
    panic!(
        "timed out waiting for {what} (deadline {deadline:?}); follower={follower_snapshot:?}; leader Hello={leader_snapshot:?}"
    );
}
