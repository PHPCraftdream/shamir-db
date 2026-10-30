use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;
use std::time::Duration;

use log::{Level, LevelFilter, Log, Metadata, Record};
use shamir_engine::tx::commit::{PostWalPrePublishHook, TEST_POST_WAL_PRE_PUBLISH_HOOK};
#[cfg(feature = "fjall")]
use shamir_engine::tx::ChangeOp;
use shamir_engine::tx::IsolationLevel;
use shamir_query_builder::write::doc;
#[cfg(feature = "fjall")]
use shamir_types::codecs::interned::inner_value_to_query_value;
use shamir_types::codecs::interned::query_value_to_inner;
use tokio::runtime::Builder;
use tokio::sync::Notify;

#[cfg(feature = "fjall")]
use crate::engine::repo::RepoInstance;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::ShamirDb;

#[cfg(feature = "fjall")]
use crate::storage::storage_fjall::FjallRepo;
#[cfg(feature = "fjall")]
use crate::storage::types::Repo;
#[cfg(feature = "fjall")]
use crate::DbError;
#[cfg(feature = "fjall")]
use bytes::Bytes;

const REPO: &str = "shutdown_quiesce_detached_tail";
const BUDGET: Duration = Duration::from_secs(45);
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static WARNINGS: WarningLog = WarningLog(Mutex::new(Vec::new()));
static LOGGER: OnceLock<()> = OnceLock::new();

struct WarningLog(Mutex<Vec<String>>);

impl Log for WarningLog {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() == Level::Warn
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let message = record.args().to_string();
            if message.contains(REPO) && message.contains("commit quiescence") {
                self.0.lock().unwrap().push(message);
            }
        }
    }

    fn flush(&self) {}
}

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(BUDGET, future)
        .await
        .expect("shutdown quiescence handshake stalled")
}

async fn pending<F: Future>(mut future: Pin<&mut F>) {
    assert!(poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx)))
        .await
        .is_pending());
}

fn hook() -> Arc<PostWalPrePublishHook> {
    TEST_POST_WAL_PRE_PUBLISH_HOOK
        .get_or_init(|| {
            Arc::new(PostWalPrePublishHook {
                repo_name: REPO.into(),
                pre_wal: AtomicBool::new(false),
                after_counter: AtomicBool::new(false),
                fail_orphan_markers: AtomicBool::new(false),
                panic_after_resume: AtomicBool::new(false),
                panic_resume: Notify::new(),
                reached: Notify::new(),
                resume: Notify::new(),
                armed: AtomicBool::new(false),
            })
        })
        .clone()
}

// Tests serialize the process-global hook and use a unique repo name.
// Unrelated parallel tests cannot consume its one-shot arm.
#[test]
fn flush_all_waits_for_detached_post_wal_tail() {
    let _serial = SERIAL.blocking_lock();
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(detached_tail_in_memory());
    drop(runtime);
}

#[cfg(feature = "fjall")]
#[test]
fn flush_all_persists_detached_post_wal_tail() {
    let _serial = SERIAL.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let (rid, value) = runtime.block_on(async {
        let hook = hook();
        let shamir = ShamirDb::init_memory().await.unwrap();
        let db = shamir.create_db("app").await.expect("create_db");
        db.add_repo(
            RepoConfig::new(REPO, BoxRepoFactory::fjall_raw(&path))
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
        let repo = db.get_repo(REPO).unwrap();
        let table = repo.get_table("items").await.unwrap();
        let interner = table.interner_for_test().get().await.unwrap();
        let value =
            query_value_to_inner(&doc().set("name", "durable").unwrap().build(), interner).unwrap();
        let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        let rid = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
        hook.armed.store(true, Ordering::Release);
        let owned = repo.clone();
        let caller = tokio::spawn(async move { owned.commit_tx(tx).await });
        within(hook.reached.notified()).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        let mut flush = Box::pin(shamir.flush_all());
        pending(flush.as_mut()).await;
        hook.resume.notify_one();
        within(flush).await.unwrap();
        repo.quiesce_commits(Duration::ZERO).await.unwrap();
        assert_eq!(table.get(rid).await.unwrap(), value);
        drop(snapshot);
        (rid, value)
    });
    // All foreground handles were dropped above. Current-thread runtime drop
    // cancels background store owners synchronously, without reopen retries.
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let repo = RepoInstance::from_factory(
            REPO.into(),
            BoxRepoFactory::fjall_raw(&path),
            vec![TableConfig::new("items")],
        )
        .await
        .expect("all previous stores must be released before reopen");
        repo.recover_v2_inflight().await.unwrap();
        let table = repo.get_table("items").await.unwrap();
        let reopened = table.get(rid).await.unwrap();
        assert_eq!(reopened, value);
        let interner = table.interner_for_test().get().await.unwrap();
        assert_eq!(
            inner_value_to_query_value(&reopened, interner).unwrap(),
            doc().set("name", "durable").unwrap().build()
        );
    });
    drop(runtime);
}

// The failed shutdown leaves J absent; cold recovery certifies surviving WAL,
// rather than declaring a blanket unclean gap.
#[cfg(feature = "fjall")]
#[test]
fn quiescence_timeout_leaves_j_absent_and_hard_reopen_certifies() {
    let _serial = SERIAL.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let (h, rid) = runtime.block_on(async {
        let hook = hook();
        let shamir = ShamirDb::init_memory().await.unwrap();
        let db = shamir.create_db("app").await.expect("create_db");
        db.add_repo(
            RepoConfig::new(REPO, BoxRepoFactory::fjall_raw(&path))
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
        let repo = db.get_repo(REPO).unwrap();
        let table = repo.get_table("items").await.unwrap();
        let interner = table.interner_for_test().get().await.unwrap();
        let value =
            query_value_to_inner(&doc().set("name", "late-tail").unwrap().build(), interner)
                .unwrap();
        let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        let rid = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
        hook.armed.store(true, Ordering::Release);
        let owned = repo.clone();
        let caller = tokio::spawn(async move { owned.commit_tx(tx).await });
        within(hook.reached.notified()).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        let error = repo.shutdown_clean(Duration::ZERO).await.unwrap_err();
        assert!(error.to_string().contains("commit quiescence"));
        hook.resume.notify_one();
        within(repo.quiesce_commits(BUDGET)).await.unwrap();
        // Drain data only; never retry clean shutdown after the failed attempt.
        repo.flush_buffers().await.unwrap();
        drop(snapshot);
        (repo.current_commit_version().await.unwrap(), rid)
    });
    drop(runtime);
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        // Inspect durable state before any opener can advance J. This proves
        // the absent marker is the precondition, not a side effect of reopen.
        let backing = FjallRepo::new(&path).unwrap();
        let store = backing.store_get("__changelog__").await.unwrap();
        assert!(matches!(
            store
                .get(Bytes::from_static(b"\0journal-clean-v1").into())
                .await,
            Err(DbError::NotFound(_))
        ));
    });
    drop(runtime);
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let repo = RepoInstance::from_factory(
            REPO.into(),
            BoxRepoFactory::fjall_raw(&path),
            vec![TableConfig::new("items")],
        )
        .await
        .unwrap();
        assert_eq!(repo.clean_horizon().await.unwrap(), h);
        assert_eq!(
            h, 1,
            "the single fresh commit advances runtime version once"
        );
        assert_eq!(repo.current_commit_version().await.unwrap(), h);
        assert_eq!(repo.consistent_version().await.unwrap(), h);
        let from_h = repo.read_changelog_from(h, 100).await.unwrap();
        assert_eq!(from_h.gap_at, None);
        let beyond_h = repo.read_changelog_from(h + 1, 100).await.unwrap();
        assert_eq!(beyond_h.gap_at, None);
        assert!(beyond_h.events.is_empty());
        assert_eq!(from_h.events.len(), 1);
        assert_eq!(from_h.events[0].commit_version, h);
        assert_eq!(from_h.events[0].changes.len(), 1);
        assert_eq!(from_h.events[0].changes[0].op, ChangeOp::Put);
        assert_eq!(from_h.events[0].changes[0].table, "items");
        assert_eq!(from_h.events[0].changes[0].key.as_ref(), rid.as_bytes());
        let table = repo.get_table("items").await.unwrap();
        let reopened = table.get(rid).await.unwrap();
        let reopened_bytes = reopened.to_bytes().unwrap();
        assert_eq!(
            from_h.events[0].changes[0].value.as_deref(),
            Some(reopened_bytes.as_ref())
        );
        let interner = table.interner_for_test().get().await.unwrap();
        assert_eq!(
            inner_value_to_query_value(&reopened, interner).unwrap(),
            doc().set("name", "late-tail").unwrap().build()
        );
    });
    drop(runtime);
}

async fn detached_tail_in_memory() {
    let hook = hook();
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("app").await.expect("create_db");
    shamir
        .add_repo(
            "app",
            RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
    let repo = shamir.get_db("app").unwrap().get_repo(REPO).unwrap();
    let table = repo.get_table("items").await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let value =
        query_value_to_inner(&doc().set("name", "survivor").unwrap().build(), interner).unwrap();
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    repo.repo_wal().await.unwrap();
    repo.tx_gate().await.unwrap();
    hook.armed.store(true, Ordering::Release);
    let owned = repo.clone();
    let caller = tokio::spawn(async move { owned.commit_tx(tx).await });
    within(hook.reached.notified()).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(repo.quiesce_commits(Duration::ZERO).await.is_err());

    // No disk I/O or cooperative-budget yield can mask old-code completion.
    // Red before the fix: an in-memory flush completes while this tail is parked.
    let mut flush = Box::pin(tokio::task::unconstrained(shamir.flush_all()));
    pending(flush.as_mut()).await;
    assert!(repo.quiesce_commits(Duration::ZERO).await.is_err());
    hook.resume.notify_one();
    within(flush).await.unwrap();
    repo.quiesce_commits(Duration::ZERO).await.unwrap();
    assert_eq!(table.get(rid).await.unwrap(), value);
    drop(snapshot);
}

#[tokio::test]
async fn flush_all_warns_and_continues_past_wedged_tail() {
    let _serial = SERIAL.lock().await;
    LOGGER.get_or_init(|| {
        log::set_logger(&WARNINGS).expect("shutdown test logger must be installed");
        log::set_max_level(LevelFilter::Warn);
    });
    WARNINGS.0.lock().unwrap().clear();
    let hook = hook();
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("app").await.expect("create_db");
    shamir
        .add_repo(
            "app",
            RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
    let repo = shamir.get_db("app").unwrap().get_repo(REPO).unwrap();
    let table = repo.get_table("items").await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let value =
        query_value_to_inner(&doc().set("name", "wedged").unwrap().build(), interner).unwrap();
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    hook.armed.store(true, Ordering::Release);
    let owned = repo.clone();
    let caller = tokio::spawn(async move { owned.commit_tx(tx).await });
    within(hook.reached.notified()).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());

    // ZERO is a deterministic expired deadline, not a wall-clock assertion.
    // Red before the fix: no quiescence warning is emitted at all.
    // A timed-out tail must not advance J even if it later resumes.
    // Hard-stop/reopen controls live in clean_horizon_tests.
    within(shamir.flush_all_with_quiesce_timeout(Duration::ZERO))
        .await
        .unwrap_err();
    assert!(repo.quiesce_commits(Duration::ZERO).await.is_err());
    {
        let warnings = WARNINGS.0.lock().unwrap();
        assert_eq!(warnings.len(), 1, "expected one repo-specific warning");
        assert!(warnings[0].contains(&format!("app/{REPO}")));
        assert!(warnings[0].contains("commit quiescence"));
    }
    hook.resume.notify_one();
    within(shamir.flush_all()).await.unwrap();
    assert_eq!(table.get(rid).await.unwrap(), value);
    drop(snapshot);
}
