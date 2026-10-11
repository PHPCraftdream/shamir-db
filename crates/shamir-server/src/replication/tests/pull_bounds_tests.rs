use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::ShamirDb;
use shamir_query_types::wire::repl::ReplResponse;

use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;

// Red before the fix: u32::MAX is passed to range_from's raw reservation,
// even though the journal is empty.
#[tokio::test]
async fn maximum_pull_limit_on_empty_journal_returns_bounded_payload() {
    let db = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(principal64_from_username("alice"));
    db.create_db_as("app", owner.clone()).await;
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
        owner,
    )
    .await
    .unwrap();
    let source = InProcessReplSource::new(db);
    let response = source.pull("app", "main", 0, u32::MAX, None).await.unwrap();
    match response {
        ReplResponse::Pull { events, gap_at, .. } => {
            assert_eq!(events, vec![0x90]);
            assert_eq!(gap_at, None);
        }
        response => panic!("unexpected Pull response: {response:?}"),
    }
}
