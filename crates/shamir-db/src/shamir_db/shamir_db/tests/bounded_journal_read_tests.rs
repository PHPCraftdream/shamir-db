use shamir_engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_engine::VisibleOutcome;

use crate::ShamirDb;

// Baseline exposes only Option<JournalRead>, collapsing errors and absence,
// and has no completion-progress facade for an idle/empty repository.
#[tokio::test]
async fn bounded_journal_wrappers_distinguish_missing_and_present_repo() {
    let db = ShamirDb::init_memory().await.unwrap();
    assert!(db
        .read_changelog_bounded("missing-db", "main", 1, 10)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        db.await_changelog_progress("missing-db", "main", 1)
            .await
            .unwrap(),
        None,
    );
    db.create_db("journal-db").await;
    assert!(db
        .read_changelog_bounded("journal-db", "missing-repo", 1, 10)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        db.await_changelog_progress("journal-db", "missing-repo", 1)
            .await
            .unwrap(),
        None,
    );
    db.add_repo(
        "journal-db",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    let page = db
        .read_changelog_bounded("journal-db", "main", 1, 10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((page.sampled_w, page.bound, page.confirmed), (0, 0, true));
    assert!(page.read.events.is_empty());
    assert_eq!(page.read.gap_at, None);
    assert_eq!(
        db.await_changelog_progress("journal-db", "main", 0)
            .await
            .unwrap(),
        Some(VisibleOutcome::Visible),
    );
    // `Ok(None)` means only that the db/repo is absent; real journal errors
    // propagate as `Err` instead of being swallowed into `Ok(None)`.
    let legacy = db
        .read_changelog_from_journal("journal-db", "main", 1, 10)
        .await
        .unwrap()
        .expect("repo exists");
    assert_eq!(legacy.events, page.read.events);
    assert_eq!(legacy.gap_at, page.read.gap_at);
}
