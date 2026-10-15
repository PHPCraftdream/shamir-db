//! S5c-X PR8b — the journal-drain source a subscription bridge reads through
//! (`DbJournalSource::read` → `ShamirDb::read_changelog_bounded`) fails
//! closed once the repo's downstream subscription is flipped to
//! `resync_required`: `JournalDrain::run` propagates that `Err`
//! (`src/subscriptions/journal_drain.rs`), and `bridge_task` logs "journal
//! subscription terminated" and exits (`src/subscriptions/bridge.rs`).
//! `DbJournalSource` itself is `pub(super)`, so this test proves the
//! mechanism at the exact public call the drain makes:
//! `ShamirDb::read_changelog_bounded` must be `Ok` before the quarantine
//! flip and `Err` (`resync_required`) after it. Fixture mirrors shamir-db's
//! `quarantine_reads_tests.rs` (profile + subscription batch, then
//! `mark_subscription_resync_required` — the real persisted path).

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_types::admin::{ReplDirection, ReplMode};

/// In-memory `ShamirDb` with db "app" and repo "main" (one "items" table) —
/// same fixture style as `bridge_snapshot_cut_tests.rs::database`.
async fn database() -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    db.add_repo_as("app", cfg, owner).await.expect("add repo");
    db
}

/// Create a Pull profile + ACTIVE subscription covering `app/main`, then
/// flip it to `resync_required` through the real persisted path.
async fn quarantine(db: &ShamirDb) {
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "profile",
        replication_profile("prof").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    db.execute_as(Actor::User(principal64([0xAB; 16])), "app", &b.build())
        .await
        .expect("admin replication batch must succeed");
    db.mark_subscription_resync_required("sub", 7, 3)
        .await
        .expect("mark resync_required must succeed");
}

/// The journal drain's read source is `Ok` while the repo is healthy and
/// fails closed with `resync_required` the moment the subscription is
/// quarantined — so a bridge attached BEFORE the quarantine terminates on
/// its next drain read.
#[tokio::test]
async fn journal_drain_read_fails_closed_after_quarantine_flip() {
    let db = database().await;

    // Healthy: the drain source read succeeds (journal attached, may be
    // `Some` with zero events on a fresh repo).
    let page = db
        .read_changelog_bounded("app", "main", 1, 10)
        .await
        .expect("journal read must succeed while healthy");
    assert!(page.is_some(), "fresh repo journal must be readable");

    quarantine(&db).await;

    let err = db
        .read_changelog_bounded("app", "main", 1, 10)
        .await
        .expect_err("journal read must be refused once quarantined");
    assert_eq!(
        err.code(),
        Some("resync_required"),
        "drain source must fail closed with resync_required"
    );
}
