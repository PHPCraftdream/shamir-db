//! Wire-dispatched Pull characterization with an authorized repo-owner session.
//! These pass existing behavior; removing bounds or breaking resume must fail.

use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::ChangelogEvent;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::db_handler::handler::ShamirDbHandler;
use crate::replication::in_process::MAX_PULL_EVENTS;

const BYTE_BUDGET: usize = 16 * 1024 * 1024;
const READINESS_ATTEMPTS: usize = 10_000;

fn owner_session() -> Session {
    // The fixture owner uses this directory id, not a username-derived id.
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(false, true, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        0,
    )
}

async fn committed_fixture(
    n: usize,
    payload_bytes: usize,
) -> (ShamirDbHandler, Vec<ChangelogEvent>) {
    let db = ShamirDb::init_memory().await.expect("init memory db");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone()).await;
    let config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    db.add_repo_as("app", config, owner.clone())
        .await
        .expect("add repo");
    let payload = "x".repeat(payload_bytes);
    let mut committed = Vec::with_capacity(n);
    for i in 0..n {
        let mut batch = Batch::named("insert");
        batch.transactional();
        batch.insert(
            "row",
            insert("items").rows([doc! {
                "id" => format!("k{i}"),
                "payload" => payload.clone(),
            }]),
        );
        let response = db
            .execute_as(owner.clone(), "app", &batch.build())
            .await
            .expect("commit row");
        assert!(
            !response.results.contains_key("__error"),
            "fixture commit failed: {response:?}",
        );
        let version = db
            .current_commit_version("app", "main")
            .await
            .expect("committed version");
        // Exact-version readiness avoids both capped-tail polling and overflow.
        let mut landed = None;
        for _ in 0..READINESS_ATTEMPTS {
            let journal = db
                .read_changelog_from_journal("app", "main", version, 1)
                .await
                .expect("journal available");
            assert_eq!(journal.gap_at, None, "fixture journal gap");
            if let Some(event) = journal.events.into_iter().next() {
                assert_eq!(event.commit_version, version);
                assert_eq!(event.repo, "main");
                assert_eq!(event.changes.len(), 1, "one row per commit");
                assert!(event.changes[0].value.as_ref().expect("put value").len() >= payload_bytes,);
                landed = Some(event);
                break;
            }
            tokio::task::yield_now().await;
        }
        committed.push(landed.expect("committed event did not land within readiness bound"));
    }
    assert!(committed
        .windows(2)
        .all(|w| w[0].commit_version < w[1].commit_version));
    (ShamirDbHandler::new(Arc::new(db)), committed)
}

async fn wire_pull(
    handler: &ShamirDbHandler,
    session: &Session,
    from_version: u64,
    current: u64,
) -> Vec<ChangelogEvent> {
    // Replication has typed DTOs, not a batch-query builder operation.
    let request = DbRequest::Repl(ReplRequest::Pull {
        db: "app".into(),
        repo: "main".into(),
        from_version,
        limit: u32::MAX,
        wait_ms: None,
        capabilities: None,
    });
    let bytes = rmp_serde::to_vec_named(&request).expect("encode wire Pull");
    let connection = ConnectionServices::without_push(1);
    let reply = handler
        .handle(session, &bytes, &connection)
        .await
        .expect("dispatch wire Pull");
    let response: DbResponse = rmp_serde::from_slice(&reply).expect("decode wire response");
    match response {
        DbResponse::Repl(ReplResponse::Pull {
            leader_epoch,
            events,
            gap_at,
            current_version,
            ..
        }) => {
            assert_eq!(leader_epoch, 1);
            assert_eq!(gap_at, None);
            assert_eq!(current_version, current);
            rmp_serde::from_slice(&events).expect("decode Pull events")
        }
        other => panic!("expected authorized wire Pull, got {other:?}"),
    }
}

// Budget compact journal entries, not the named response or transport bytes.
fn journal_bytes(events: &[ChangelogEvent]) -> usize {
    events
        .iter()
        .map(|event| {
            rmp_serde::to_vec(event)
                .expect("encode journal entry")
                .len()
        })
        .sum()
}

#[tokio::test]
async fn wire_oversized_event_limit_returns_ordered_cap_and_resumes() {
    let (handler, committed) = committed_fixture(2000, 0).await;
    let session = owner_session();
    let current = committed.last().expect("commits").commit_version;
    let expected = &committed[7..];
    let first = wire_pull(&handler, &session, expected[0].commit_version, current).await;
    let cap = usize::try_from(MAX_PULL_EVENTS).expect("cap fits usize");
    assert_eq!(
        first.len(),
        cap,
        "ready small events must fill the capped page",
    );
    assert_eq!(first, expected[..cap], "inclusive start and ordered prefix");
    let resume = first.last().expect("nonempty page").commit_version + 1;
    let second = wire_pull(&handler, &session, resume, current).await;
    assert_eq!(
        second,
        expected[cap..],
        "resume without loss or duplication"
    );
}

#[tokio::test]
async fn wire_byte_budget_returns_nonempty_prefix_and_resumes() {
    // 20 MiB across ten events: byte budget binds independently of event cap.
    let (handler, committed) = committed_fixture(10, 2 * 1024 * 1024).await;
    assert!(journal_bytes(&committed) > BYTE_BUDGET);
    let session = owner_session();
    let current = committed.last().expect("commits").commit_version;
    let first = wire_pull(&handler, &session, committed[0].commit_version, current).await;
    assert!(!first.is_empty());
    assert!(
        first.len() < committed.len(),
        "byte budget must truncate the tail",
    );
    let used = journal_bytes(&first);
    assert!(used < BYTE_BUDGET, "journal bytes {used} exceed budget");
    assert_eq!(first, committed[..first.len()], "ordered resumable prefix");
    assert!(
        used + journal_bytes(&committed[first.len()..first.len() + 1]) > BYTE_BUDGET,
        "prefix must stop at the first entry that would exceed the budget",
    );
    let resume = first.last().expect("nonempty page").commit_version + 1;
    let second = wire_pull(&handler, &session, resume, current).await;
    assert!(journal_bytes(&second) <= BYTE_BUDGET);
    assert_eq!(
        second,
        committed[first.len()..],
        "resume without loss or duplication",
    );
}
