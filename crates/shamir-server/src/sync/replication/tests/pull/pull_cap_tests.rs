//! Characterization of Pull bounds on real committed single-row transactions.
//! Removing the event clamp or journal byte budget must break these tests.

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::ChangelogEvent;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::wire::repl::ReplResponse;

use crate::replication::in_process::{InProcessReplSource, MAX_PULL_EVENTS};
use crate::replication::source::ReplSource;

const BYTE_BUDGET: usize = 16 * 1024 * 1024;
const READINESS_ATTEMPTS: usize = 10_000;

async fn committed_fixture(n: usize, payload_bytes: usize) -> (ShamirDb, Vec<ChangelogEvent>) {
    let db = ShamirDb::init_memory().await.expect("init memory db");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
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
        // Observe each exact committed version, not a capped full-tail read.
        // This also drains the writer between commits, avoiding channel overflow.
        let mut landed = None;
        for _ in 0..READINESS_ATTEMPTS {
            let journal = db
                .read_changelog_from_journal("app", "main", version, 1)
                .await
                .expect("journal read")
                .expect("journal present");
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
    (db, committed)
}

fn decode_pull(response: ReplResponse, current: u64) -> Vec<ChangelogEvent> {
    match response {
        ReplResponse::Pull {
            leader_epoch,
            events,
            gap_at,
            current_version,
            ..
        } => {
            assert_eq!(leader_epoch, 1);
            assert_eq!(gap_at, None);
            assert_eq!(current_version, current);
            rmp_serde::from_slice(&events).expect("decode Pull events")
        }
        other => panic!("expected Pull, got {other:?}"),
    }
}

// The journal budgets compact per-entry encoding, NOT the named wire envelope.
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
async fn oversized_event_limit_returns_ordered_cap_and_resumes() {
    let (db, committed) = committed_fixture(2000, 0).await;
    let current = committed.last().expect("commits").commit_version;
    let source = InProcessReplSource::new(db);
    // Nonzero inclusive starting version, leaving more than one capped page.
    let expected = &committed[7..];
    let first = decode_pull(
        source
            .pull("app", "main", expected[0].commit_version, u32::MAX, None)
            .await
            .expect("first Pull"),
        current,
    );
    let cap = usize::try_from(MAX_PULL_EVENTS).expect("cap fits usize");
    assert_eq!(
        first.len(),
        cap,
        "ready small events must fill the capped page",
    );
    assert_eq!(first, expected[..cap], "inclusive start and ordered prefix");
    let resume = first.last().expect("nonempty page").commit_version + 1;
    let second = decode_pull(
        source
            .pull("app", "main", resume, u32::MAX, None)
            .await
            .expect("resume Pull"),
        current,
    );
    assert_eq!(
        second,
        expected[cap..],
        "resume without loss or duplication"
    );
}

#[tokio::test]
async fn byte_budget_returns_nonempty_prefix_and_resumes() {
    // 20 MiB across ten events: the byte budget, not the event cap, binds.
    let (db, committed) = committed_fixture(10, 2 * 1024 * 1024).await;
    assert!(journal_bytes(&committed) > BYTE_BUDGET);
    let current = committed.last().expect("commits").commit_version;
    let source = InProcessReplSource::new(db);
    let first = decode_pull(
        source
            .pull("app", "main", committed[0].commit_version, u32::MAX, None)
            .await
            .expect("first Pull"),
        current,
    );
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
    let second = decode_pull(
        source
            .pull("app", "main", resume, u32::MAX, None)
            .await
            .expect("resume Pull"),
        current,
    );
    assert!(journal_bytes(&second) <= BYTE_BUDGET);
    assert_eq!(
        second,
        committed[first.len()..],
        "resume without loss or duplication",
    );
}
