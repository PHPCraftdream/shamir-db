//! S5c-X PR7 — tests for the universal leader-identity fence
//! ([`identity_fence`]).
//!
//! Scenarios:
//!   1. first contact records identity + bookmark together (one atomic step);
//!   2. the record survives a follower restart (durable follower);
//!   3. lineage mismatch on a PULL stops the loop before applying anything;
//!   4. history-epoch mismatch on a PULL does the same;
//!   5. lineage mismatch on the HELLO stops the loop before any pull;
//!   6. a pre-existing subscription without a record adopts exactly once and
//!      keeps applying;
//!   7. a regenerated (restarted) in-memory leader stops its follower;
//!   8. NEGATIVE CONTROL: with the fence disabled through the explicit test
//!      seam the follower of the regenerated leader keeps applying;
//!   9. a restarted DURABLE (fjall) leader keeps its identity and its
//!      follower keeps applying;
//!  10. through the supervisor, the subscription status shows the typed
//!      reason plus the recorded and observed identities, and the loop does
//!      not restart.
//!
//! No wall-clock waits for conditions the fence itself enforces: every
//! mismatch terminates the loop synchronously; journal confirmation precedes
//! a bounded foreground single-batch follower run.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::meta::recovery_marker::load_last_committed;
use shamir_db::engine::meta::repl::bookmark_v3::decode_bookmark_v3;
use shamir_db::engine::meta::repo_identity::RepoIdentity;
use shamir_db::engine::meta::MetaKey;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::storage::types::RecordKey;
use shamir_db::types::value::QueryValue;
use shamir_db::SeedRefusal;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    repl_scope, replication_profile, replication_status, subscription,
};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::identity_fence::checks::hex_lineage;
use crate::replication::identity_fence::recorded::{self, FenceRecord};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, SubscriptionSupervisor};

#[tokio::test]
async fn first_contact_on_fresh_atomic_repo_adopts_v3_before_apply() {
    let leader = build_memory().await;
    write_rows(&leader, 0, 1).await;
    let follower = build_memory().await;
    let source = Arc::new(InProcessReplSource::new(leader));
    run_follower_loop(
        Arc::new(follower.clone()),
        source,
        FollowerLoopConfig::new("fresh-v3", "app", "main")
            .with_poll_wait_ms(0)
            .with_max_iterations(1),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let info = repo(&follower).tx_info_store().await.unwrap();
    let raw = info
        .get(RecordKey::from_slice(
            MetaKey::ReplicationBookmark.as_record_id().as_bytes(),
        ))
        .await
        .unwrap();
    let v3 = decode_bookmark_v3(&raw).unwrap();
    assert!(v3.fence.is_some(), "identity adoption must be in v3");
    assert!(
        v3.bookmark > 0,
        "the subsequent apply must advance bookmark"
    );
    assert_eq!(v3.clean_through, Some(v3.bookmark));
    assert_eq!(load_last_committed(&info).await.unwrap(), Some(v3.bookmark));
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------
/// In-memory `ShamirDb` with db `app`, repo `main`, table `items`.
pub(super) async fn build_memory() -> ShamirDb {
    build_factory(BoxRepoFactory::in_memory()).await
}

pub(super) async fn build_factory(factory: BoxRepoFactory) -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64_from_username("alice"));
    shamir
        .create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    let cfg = RepoConfig::new("main", factory).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner)
        .await
        .expect("add repo");
    shamir
}

/// Durable (fjall) repo over `path` inside a fresh catalogue; reopening the
/// same `path` yields a NEW catalogue over the SAME data repo.
async fn build_durable(path: Option<&std::path::Path>) -> ShamirDb {
    let factory = match path {
        Some(path) => BoxRepoFactory::fjall_raw(path),
        None => BoxRepoFactory::in_memory(),
    };
    build_factory(factory).await
}

pub(super) fn repo(db: &ShamirDb) -> shamir_db::engine::repo::RepoInstance {
    db.get_db("app")
        .expect("db")
        .get_repo("main")
        .expect("repo")
}

/// Write `n` rows, confirm the journal prefix, then flush buffers.
pub(super) async fn write_rows(leader: &ShamirDb, base: usize, n: usize) {
    let owner = Actor::User(principal64_from_username("alice"));
    let before = journal_len(leader).await;
    for i in base..(base + n) {
        let mut batch = Batch::named("ins");
        batch.id("ins");
        batch.transactional();
        batch.insert(
            "i",
            insert("items").rows([doc! {
                "id" => format!("k{i}"),
                "v" => i as i64,
            }]),
        );
        let resp = leader
            .execute_as(owner.clone(), "app", &batch.build())
            .await
            .expect("fixture write");
        assert!(!resp.results.contains_key("__error"), "{resp:?}");
    }
    let read = repo(leader).read_changelog_bounded(0, 1000).await.unwrap();
    assert!(
        read.confirmed,
        "journal barrier must confirm the completion prefix"
    );
    assert_eq!(read.read.gap_at, None);
    assert_eq!(read.read.events.len(), before + n);
    repo(leader).flush_buffers().await.unwrap();
}

async fn journal_len(db: &ShamirDb) -> usize {
    let read = repo(db).read_changelog_bounded(0, 1000).await.unwrap();
    assert!(read.confirmed);
    assert_eq!(read.read.gap_at, None);
    read.read.events.len()
}

async fn bookmark_of(db: &ShamirDb) -> u64 {
    repo(db).replication_bookmark().await.expect("bookmark")
}

async fn fence_of(db: &ShamirDb) -> Option<FenceRecord> {
    let info = repo(db).tx_info_store().await.expect("info store");
    recorded::load(&info).await.expect("fence record read")
}

/// One confirmed batch, run in the foreground; no cancellation race.
async fn catch_up(follower: &ShamirDb, leader: &ShamirDb, target: u64) {
    let read = repo(leader).read_changelog_bounded(0, 1000).await.unwrap();
    assert!(read.confirmed);
    repo(leader).flush_buffers().await.unwrap();
    run_bounded(
        follower,
        Arc::new(InProcessReplSource::new(leader.clone())),
        1,
    )
    .await
    .expect("one batch applies");
    assert_eq!(bookmark_of(follower).await, target);
}

/// One bounded foreground loop run.
async fn run_bounded(
    follower: &ShamirDb,
    source: Arc<dyn ReplSource>,
    iterations: usize,
) -> Result<(), ReplError> {
    let cfg = FollowerLoopConfig::new("fence-test", "app", "main")
        .with_poll_wait_ms(0)
        .with_max_iterations(iterations.min(1));
    run_follower_loop(
        Arc::new(follower.clone()),
        source,
        cfg,
        CancellationToken::new(),
    )
    .await
}

/// Test seam: serve a FIXED Hello (e.g. the one captured before a leader
/// regeneration) while Pulls are served live by `inner`. This is how the
/// Hello path and the Pull path of the fence are exercised independently.
pub(super) struct StaticHelloSource {
    pub(super) hello: ReplResponse,
    pub(super) inner: Arc<InProcessReplSource>,
    pub(super) pulls: AtomicUsize,
}

#[async_trait]
impl ReplSource for StaticHelloSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        Ok(self.hello.clone())
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        self.pulls.fetch_add(1, Ordering::AcqRel);
        self.inner
            .pull(db, repo, from_version, limit, wait_ms)
            .await
    }
}

pub(super) fn assert_identity_mismatch(error: &ReplError, want: SeedRefusal) {
    match error {
        ReplError::IdentityMismatch { code, .. } => {
            assert_eq!(*code, want, "{error}");
        }
        other => panic!("expected identity mismatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 1 + 2 — first contact records identity + bookmark together; survives restart
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_contact_records_identity_with_bookmark_atomically() {
    let leader = build_memory().await;
    write_rows(&leader, 0, 2).await;
    let leader_version = leader.current_commit_version("app", "main").await.unwrap();

    let follower = build_memory().await;
    run_bounded(
        &follower,
        Arc::new(InProcessReplSource::new(leader.clone())),
        1,
    )
    .await
    .expect("first iteration applies the first batch");

    let record = fence_of(&follower).await.expect("identity recorded");
    let identity: RepoIdentity = repo(&leader).repo_identity().await.unwrap();
    assert_eq!(record.lineage_id, identity.lineage_id);
    assert_eq!(record.history_epoch, identity.history_epoch);
    // The SAME record advances with the bookmark and retains identity.
    assert_eq!(record.bookmark, bookmark_of(&follower).await);
    assert!(
        bookmark_of(&follower).await >= leader_version,
        "the first batch was applied after the identity was recorded"
    );
    let reloaded = fence_of(&follower).await.expect("record still present");
    assert_eq!(reloaded, record);
}

#[test]
fn fence_record_survives_follower_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("follower");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (identity, bookmark) = rt.block_on(async {
        let leader = build_memory().await;
        write_rows(&leader, 0, 2).await;
        let target = leader.current_commit_version("app", "main").await.unwrap();
        let follower = build_durable(Some(&path)).await;
        catch_up(&follower, &leader, target).await;
        let record = fence_of(&follower).await.expect("record after catch-up");
        (record, bookmark_of(&follower).await)
    });
    // The runtime drop releases the durable stores; the record must be on
    // disk together with the advanced bookmark.
    drop(rt);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let follower = build_durable(Some(&path)).await;
        let reloaded = fence_of(&follower).await.expect("record survives restart");
        assert_eq!(reloaded, identity);
        assert!(bookmark_of(&follower).await >= bookmark);
    });
}

// ---------------------------------------------------------------------------
// 3 + 4 — lineage / epoch mismatch on a PULL applies nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lineage_mismatch_on_pull_stops_before_applying() {
    let leader_a = build_memory().await;
    write_rows(&leader_a, 0, 2).await;
    let caught_up = leader_a
        .current_commit_version("app", "main")
        .await
        .unwrap();
    let follower = build_memory().await;
    catch_up(&follower, &leader_a, caught_up).await;

    // A regenerated leader: same schema, DIFFERENT lineage.
    let leader_b = build_memory().await;
    write_rows(&leader_b, 10, 4).await;

    let hello_a = InProcessReplSource::new(leader_a.clone())
        .hello("probe")
        .await
        .expect("hello from A");
    let source = Arc::new(StaticHelloSource {
        hello: hello_a,
        inner: Arc::new(InProcessReplSource::new(leader_b.clone())),
        pulls: AtomicUsize::new(0),
    });
    let journal_before = journal_len(&follower).await;
    let local_before = repo(&follower).current_commit_version().await.unwrap();
    let error = run_bounded(&follower, source, 5).await.unwrap_err();
    assert_eq!(
        repo(&follower).current_commit_version().await.unwrap(),
        local_before
    );
    assert_identity_mismatch(&error, SeedRefusal::WrongIdentity);
    assert_eq!(
        bookmark_of(&follower).await,
        caught_up,
        "bookmark must not advance past the recorded identity"
    );
    assert_eq!(
        journal_len(&follower).await,
        journal_before,
        "zero events from the mismatching leader may be applied"
    );
}

#[tokio::test]
async fn epoch_mismatch_on_pull_stops_before_applying() {
    let leader = build_memory().await;
    write_rows(&leader, 0, 2).await;
    let caught_up = leader.current_commit_version("app", "main").await.unwrap();
    let follower = build_memory().await;
    catch_up(&follower, &leader, caught_up).await;

    // Capture the Hello BEFORE the epoch bump so the loop reaches the Pull.
    let hello_before = InProcessReplSource::new(leader.clone())
        .hello("probe")
        .await
        .expect("hello before bump");
    repo(&leader).bump_history_epoch().await.unwrap();
    write_rows(&leader, 2, 1).await;

    let source = Arc::new(StaticHelloSource {
        hello: hello_before,
        inner: Arc::new(InProcessReplSource::new(leader.clone())),
        pulls: AtomicUsize::new(0),
    });
    let journal_before = journal_len(&follower).await;
    let local_before = repo(&follower).current_commit_version().await.unwrap();
    let error = run_bounded(&follower, source, 5).await.unwrap_err();
    assert_eq!(
        repo(&follower).current_commit_version().await.unwrap(),
        local_before
    );
    assert_identity_mismatch(&error, SeedRefusal::WrongHistoryEpoch);
    assert_eq!(bookmark_of(&follower).await, caught_up);
    assert_eq!(journal_len(&follower).await, journal_before);
}

// ---------------------------------------------------------------------------
// 5 — lineage mismatch on the HELLO stops before any pull
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hello_mismatch_stops_before_any_pull() {
    let leader_a = build_memory().await;
    write_rows(&leader_a, 0, 1).await;
    let caught_up = leader_a
        .current_commit_version("app", "main")
        .await
        .unwrap();
    let follower = build_memory().await;
    catch_up(&follower, &leader_a, caught_up).await;

    // Pulls would MATCH (served by A); only the Hello mismatches (from B).
    let leader_b = build_memory().await;
    let source = Arc::new(StaticHelloSource {
        hello: InProcessReplSource::new(leader_b.clone())
            .hello("probe")
            .await
            .expect("hello from B"),
        inner: Arc::new(InProcessReplSource::new(leader_a.clone())),
        pulls: AtomicUsize::new(0),
    });
    let local_before = repo(&follower).current_commit_version().await.unwrap();
    let error = run_bounded(&follower, source.clone(), 5).await.unwrap_err();
    assert_eq!(
        repo(&follower).current_commit_version().await.unwrap(),
        local_before
    );
    assert_identity_mismatch(&error, SeedRefusal::WrongIdentity);
    assert_eq!(
        source.pulls.load(Ordering::Acquire),
        0,
        "the loop must stop at the Hello, before the first pull"
    );
    assert_eq!(bookmark_of(&follower).await, caught_up);
}

// ---------------------------------------------------------------------------
// 6 — pre-existing subscription adopts exactly once and keeps applying
// ---------------------------------------------------------------------------

#[tokio::test]
async fn legacy_subscription_adopts_identity_once_and_continues() {
    let leader = build_memory().await;
    write_rows(&leader, 0, 2).await;
    let first_target = leader.current_commit_version("app", "main").await.unwrap();
    let follower = build_memory().await;

    // A pre-PR7 subscription: events applied, bookmark advanced, NO record.
    let cfg = FollowerLoopConfig::new("legacy", "app", "main")
        .with_poll_wait_ms(0)
        .with_max_iterations(1)
        .with_identity_fence(false);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(InProcessReplSource::new(leader.clone())),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("legacy loop applies");
    assert!(bookmark_of(&follower).await >= first_target);
    assert!(fence_of(&follower).await.is_none(), "nothing recorded yet");

    // The next run adopts the identity exactly once — and CONTINUES.
    write_rows(&leader, 2, 2).await;
    let second_target = leader.current_commit_version("app", "main").await.unwrap();
    run_bounded(
        &follower,
        Arc::new(InProcessReplSource::new(leader.clone())),
        5,
    )
    .await
    .expect("adoption must not stop the loop");
    let record = fence_of(&follower).await.expect("adopted exactly once");
    let identity = repo(&leader).repo_identity().await.unwrap();
    assert_eq!(record.lineage_id, identity.lineage_id);
    assert_eq!(record.history_epoch, identity.history_epoch);
    assert!(
        bookmark_of(&follower).await >= second_target,
        "adoption must not block applying"
    );
}

// ---------------------------------------------------------------------------
// 7 + 8 — regenerated in-memory leader: fence stops; seam-disabling applies
// ---------------------------------------------------------------------------

async fn regenerate_in_memory_leader(rows_base: usize) -> ShamirDb {
    let leader_b = build_memory().await;
    write_rows(&leader_b, rows_base, 4).await;
    leader_b
}

#[tokio::test]
async fn restarted_in_memory_leader_stops_follower() {
    let leader_a = build_memory().await;
    write_rows(&leader_a, 0, 2).await;
    let caught_up = leader_a
        .current_commit_version("app", "main")
        .await
        .unwrap();
    let follower = build_memory().await;
    catch_up(&follower, &leader_a, caught_up).await;

    // "Restart": a fresh in-memory instance mints a NEW lineage (its
    // `__tx__` identity record is RAM-only).
    let restarted = regenerate_in_memory_leader(10).await;
    let restarted_identity = repo(&restarted).repo_identity().await.unwrap();
    let recorded_identity = repo(&leader_a).repo_identity().await.unwrap();
    assert_ne!(
        restarted_identity.lineage_id, recorded_identity.lineage_id,
        "a regenerated in-memory leader must have a new lineage"
    );

    let journal_before = journal_len(&follower).await;
    let local_before = repo(&follower).current_commit_version().await.unwrap();
    let error = run_bounded(
        &follower,
        Arc::new(InProcessReplSource::new(restarted.clone())),
        5,
    )
    .await
    .unwrap_err();
    assert_eq!(
        repo(&follower).current_commit_version().await.unwrap(),
        local_before
    );
    assert_identity_mismatch(&error, SeedRefusal::WrongIdentity);
    assert_eq!(bookmark_of(&follower).await, caught_up);
    assert_eq!(journal_len(&follower).await, journal_before);
}

/// NEGATIVE CONTROL: with the universal fence disabled through the explicit
/// test seam, the follower of the regenerated in-memory leader KEEPS
/// APPLYING. Without the fence this is exactly the silent-history-swap bug
/// PR7 closes (this test's expectation inverts the fence test above).
#[tokio::test]
async fn fence_seam_disabled_follower_of_regenerated_leader_keeps_applying() {
    let leader_a = build_memory().await;
    write_rows(&leader_a, 0, 2).await;
    let caught_up = leader_a
        .current_commit_version("app", "main")
        .await
        .unwrap();
    let follower = build_memory().await;
    catch_up(&follower, &leader_a, caught_up).await;

    let restarted = regenerate_in_memory_leader(10).await;
    let restarted_target = restarted
        .current_commit_version("app", "main")
        .await
        .unwrap();
    let journal_before = journal_len(&follower).await;
    let local_before = repo(&follower).current_commit_version().await.unwrap();

    let cfg = FollowerLoopConfig::new("fence-test", "app", "main")
        .with_poll_wait_ms(0)
        .with_max_iterations(1)
        .with_identity_fence(false);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(InProcessReplSource::new(restarted)),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("without the fence the loop keeps running");

    assert!(
        bookmark_of(&follower).await >= restarted_target,
        "unfenced follower applied the regenerated leader's events"
    );
    assert!(
        repo(&follower).current_commit_version().await.unwrap() > local_before
            && journal_len(&follower).await > journal_before,
        "unfenced follower's changefeed grew from the swapped history"
    );
}

// ---------------------------------------------------------------------------
// 9 — a restarted DURABLE leader keeps its identity and its follower
// ---------------------------------------------------------------------------

#[test]
fn restarted_durable_leader_keeps_follower_running() {
    let dir = tempfile::tempdir().unwrap();
    let leader_path = dir.path().join("leader");
    let follower_path = dir.path().join("follower");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let pre_record = rt.block_on(async {
        let leader = build_durable(Some(&leader_path)).await;
        write_rows(&leader, 0, 2).await;
        let target = leader.current_commit_version("app", "main").await.unwrap();
        let follower = build_durable(Some(&follower_path)).await;
        catch_up(&follower, &leader, target).await;
        let record = fence_of(&follower).await.expect("record after catch-up");
        repo(&leader)
            .shutdown_clean(std::time::Duration::from_secs(5))
            .await
            .unwrap();
        record
    });
    // Clean leader shutdown avoids testing the independent cold-start GAP.
    drop(rt);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let leader = build_durable(Some(&leader_path)).await;
        let identity = repo(&leader).repo_identity().await.unwrap();
        assert_eq!(
            identity.lineage_id, pre_record.lineage_id,
            "a durable leader keeps its lineage across a restart"
        );
        write_rows(&leader, 2, 2).await;
        let target = leader.current_commit_version("app", "main").await.unwrap();

        // The follower reopens with its pre-restart fence record intact and
        // must keep applying — the fence must NOT trip on a durable restart.
        let follower = build_durable(Some(&follower_path)).await;
        assert_eq!(
            fence_of(&follower).await.expect("record survives"),
            pre_record,
        );
        run_bounded(
            &follower,
            Arc::new(InProcessReplSource::new(leader.clone())),
            5,
        )
        .await
        .expect("durable leader restart must NOT trip the fence");
        assert!(
            bookmark_of(&follower).await >= target,
            "follower kept applying after the leader's durable restart"
        );
    });
}

// ---------------------------------------------------------------------------
// 10 — the supervisor persists the typed reason + both identities
// ---------------------------------------------------------------------------

#[tokio::test]
async fn supervisor_marks_status_with_typed_reason_and_identities() {
    let leader_a = build_memory().await;
    write_rows(&leader_a, 0, 2).await;
    let caught_up = leader_a
        .current_commit_version("app", "main")
        .await
        .unwrap();
    let follower = Arc::new(build_memory().await);
    catch_up(&follower, &leader_a, caught_up).await;
    let recorded = fence_of(&follower).await.expect("record from leader A");

    let restarted = regenerate_in_memory_leader(10).await;
    let observed = repo(&restarted).repo_identity().await.unwrap();

    let mut batch = Batch::new();
    batch.op(
        "profile",
        replication_profile("prof_fence").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "sub",
        subscription("sub_fence", "tcp://leader:9000", "pub", "prof_fence"),
    );
    follower
        .execute("app", &batch.build())
        .await
        .expect("create profile+subscription");

    let restarted_for_factory = restarted.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| {
        Arc::new(InProcessReplSource::new(restarted_for_factory.clone())) as Arc<dyn ReplSource>
    });
    let supervisor = SubscriptionSupervisor::new(follower.clone(), factory, "follower-fence")
        .with_poll_wait_ms(0)
        .with_max_iterations(1);
    supervisor.reconcile().await;
    // Join the loop task: its return guarantees the terminal marking has
    // been persisted (no polling, no timers).
    supervisor.wait_for_subscription("sub_fence").await;

    let mut batch = Batch::new();
    batch.op("list", replication_status());
    let response = follower.execute("app", &batch.build()).await.unwrap();
    let rows = &response.results["list"].records;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let envelope = rows[0].as_value();
    let QueryValue::List(entries) = &envelope["subscriptions"] else {
        panic!("status entries must be a list: {envelope:?}");
    };
    assert_eq!(entries.len(), 1);
    let row = &entries[0];
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some("resync_required"),
        "a fenced subscription is quarantined as resync_required; the typed
         reason stays in the status message"
    );
    let message = row
        .get("status_message")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        message.contains(&hex_lineage(&recorded.lineage_id)),
        "status must expose the RECORDED identity: {message}"
    );
    assert!(
        message.contains(&hex_lineage(&observed.lineage_id)),
        "status must expose the OBSERVED identity: {message}"
    );

    assert_eq!(
        row["recorded_identity"]["history_epoch"].as_i64(),
        Some(recorded.history_epoch as i64)
    );
    assert_eq!(
        row["observed_identity"]["history_epoch"].as_i64(),
        Some(observed.history_epoch as i64)
    );
    assert_ne!(
        row["recorded_identity"]["lineage_id"],
        row["observed_identity"]["lineage_id"]
    );

    // The loop is not restarted behind the operator's back.
    supervisor.reconcile().await;
    assert_eq!(
        supervisor.active_count(),
        0,
        "a fenced subscription must not auto-restart"
    );
    supervisor.stop_all().await;
}
