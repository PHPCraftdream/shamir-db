use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Notify};

use super::super::support::{create_echo, echo_variant_wasm, echo_wasm, stored_row, tamper_row};
use crate::access::{Actor, ResourceMeta, ResourcePath};
use crate::shamir_db::shamir_db::core::FunctionDdlHookPoint;
use crate::shamir_db::shamir_db::ShamirDb;
use crate::shamir_db::FunctionState;
use crate::DbError;

type FunctionDdlAcquiredEvent = (String, tokio::task::Id);
type FunctionDdlAcquiredEvents = mpsc::Receiver<FunctionDdlAcquiredEvent>;
type FunctionDdlAcquiredNamesByTask = shamir_collections::TFxMap<tokio::task::Id, Vec<String>>;

fn install_pause(db: &ShamirDb, target: FunctionDdlHookPoint) -> (Arc<Notify>, Arc<Notify>) {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let hook_entered = Arc::clone(&entered);
    let hook_release = Arc::clone(&release);
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook_fired = Arc::clone(&fired);
    assert!(db
        .function_ddl_hook
        .set(Box::new(move |point| {
            let entered = Arc::clone(&hook_entered);
            let release = Arc::clone(&hook_release);
            let target = target.clone();
            let fired = Arc::clone(&hook_fired);
            Box::pin(async move {
                if point == target && !fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    entered.notify_one();
                    release.notified().await;
                }
            })
        }))
        .is_ok());
    (entered, release)
}

fn install_wait_notice(db: &ShamirDb) -> mpsc::Receiver<String> {
    let (send, receive) = mpsc::channel(8);
    db.function_ddl_locks.set_wait_hook(Arc::new(move |name| {
        send.try_send(name)
            .expect("bounded wait notice capacity covers test waiters");
    }));
    receive
}

fn install_acquired_notice(db: &ShamirDb) -> FunctionDdlAcquiredEvents {
    let (send, receive) = mpsc::channel(16);
    db.function_ddl_locks
        .set_acquired_hook(Arc::new(move |name, task_id| {
            send.try_send((name, task_id))
                .expect("bounded acquired notice capacity covers test acquisitions");
        }));
    receive
}

async fn assert_acquired_names(
    receiver: &mut FunctionDdlAcquiredEvents,
    task_id: tokio::task::Id,
    expected: &[&str],
) {
    let mut names = Vec::with_capacity(expected.len());
    for _ in expected {
        let (name, acquired_by) = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("lock acquisition is reported within the deadline")
            .expect("acquired notice channel remains connected");
        assert_eq!(acquired_by, task_id);
        names.push(name);
    }
    assert_eq!(
        names,
        expected
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    );
}

async fn wait_for_pause(entered: &Notify) {
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("operation reached its deterministic pause point");
}

async fn wait_for_lock_wait(receiver: &mut mpsc::Receiver<String>, name: &str) {
    let seen = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("conflicting operation reached a pending lock acquisition")
        .expect("lock wait channel remains connected");
    assert_eq!(seen, name);
}

async fn finish_pause(release: &Notify) {
    release.notify_one();
}

fn artifact_id(row: &shamir_types::types::value::QueryValue) -> &str {
    row.get("artifact_id")
        .and_then(|value| value.as_str())
        .expect("row artifact id")
}

fn binding_id(db: &ShamirDb, name: &str) -> String {
    db.functions()
        .capture(name)
        .expect("registered binding")
        .artifact_id()
        .expect("binding artifact id")
        .to_string()
}

fn assert_row_matches_binding(
    db: &ShamirDb,
    name: &str,
    row: &shamir_types::types::value::QueryValue,
) {
    assert_eq!(artifact_id(row), binding_id(db, name));
}

#[tokio::test]
async fn alter_serializes_against_owner_replace_without_rolling_back_artifact() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "alter_race").await;
    let (entered, release) = install_pause(&db, FunctionDdlHookPoint::AlterAfterRowReadBeforeWrite);
    let mut waits = install_wait_notice(&db);

    let alter_db = db.clone();
    let alter = tokio::spawn(async move {
        alter_db
            .alter_function_optimize("alter_race", shamir_engine::function::TierMode::Never)
            .await
    });
    wait_for_pause(&entered).await;

    let replace_db = db.clone();
    let replace = tokio::spawn(async move {
        replace_db
            .create_function_from_wasm("alter_race", &echo_variant_wasm(), true)
            .await
    });
    wait_for_lock_wait(&mut waits, "alter_race").await;
    finish_pause(&release).await;
    alter.await.expect("alter task").expect("alter succeeds");
    replace
        .await
        .expect("replace task")
        .expect("owner replace succeeds");

    let row = stored_row(&db, "alter_race").await;
    assert_row_matches_binding(&db, "alter_race", &row);
    assert_eq!(
        row["module_hash"].as_str(),
        Some(
            shamir_engine::function::ModuleHash::of(&echo_variant_wasm())
                .to_hex()
                .as_str()
        )
    );
    assert!(
        db.function_ddl_locks.is_empty(),
        "alter and replace release their lock leases"
    );
}

#[tokio::test]
async fn rename_serializes_against_replace_of_source_name() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "rename_source").await;
    let source_id = binding_id(&db, "rename_source");
    let (entered, release) =
        install_pause(&db, FunctionDdlHookPoint::RenameAfterRowReadBeforeWrite);
    let mut waits = install_wait_notice(&db);

    let rename_db = db.clone();
    let rename = tokio::spawn(async move {
        rename_db
            .rename_function("rename_source", "rename_target")
            .await
    });
    wait_for_pause(&entered).await;

    let replace_db = db.clone();
    let replace = tokio::spawn(async move {
        replace_db
            .create_function_from_wasm("rename_source", &echo_variant_wasm(), true)
            .await
    });
    wait_for_lock_wait(&mut waits, "rename_source").await;
    finish_pause(&release).await;
    rename.await.expect("rename task").expect("rename succeeds");
    replace
        .await
        .expect("replace task")
        .expect("source name can be recreated by replacement");

    let moved = stored_row(&db, "rename_target").await;
    assert_eq!(artifact_id(&moved), source_id);
    assert_row_matches_binding(&db, "rename_target", &moved);
    let source = stored_row(&db, "rename_source").await;
    assert_ne!(artifact_id(&source), source_id);
    assert_row_matches_binding(&db, "rename_source", &source);
    assert!(
        db.function_ddl_locks.is_empty(),
        "rename and replacement release their lock leases"
    );
}

#[tokio::test]
async fn chmod_serializes_against_replace_and_preserves_new_metadata() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "chmod_race").await;
    let initial_meta = ResourceMeta {
        owner: Actor::User(41),
        group: Some(7),
        mode: 0o740,
    };
    db.set_resource_meta(&ResourcePath::function("chmod_race"), &initial_meta)
        .await
        .expect("set initial metadata");
    let meta = ResourceMeta {
        mode: 0o750,
        ..initial_meta
    };
    let (entered, release) = install_pause(&db, FunctionDdlHookPoint::MetaAfterRowReadBeforeWrite);
    let mut waits = install_wait_notice(&db);

    let chmod_db = db.clone();
    let chmod = tokio::spawn(async move {
        chmod_db
            .set_resource_meta(&ResourcePath::function("chmod_race"), &meta)
            .await
    });
    wait_for_pause(&entered).await;

    let replace_db = db.clone();
    let replace = tokio::spawn(async move {
        replace_db
            .create_function_from_wasm("chmod_race", &echo_variant_wasm(), true)
            .await
    });
    wait_for_lock_wait(&mut waits, "chmod_race").await;
    finish_pause(&release).await;
    chmod.await.expect("chmod task").expect("chmod succeeds");
    replace
        .await
        .expect("replace task")
        .expect("owner replace succeeds");

    let row = stored_row(&db, "chmod_race").await;
    let actual = ResourceMeta::from_record(&row);
    assert_eq!(actual.owner, Actor::User(41));
    assert_eq!(actual.group, Some(7));
    assert_eq!(actual.mode, 0o750);
    assert_row_matches_binding(&db, "chmod_race", &row);
    assert!(
        db.function_ddl_locks.is_empty(),
        "chmod and replacement release their lock leases"
    );
}

#[tokio::test]
async fn stale_corruption_detection_cannot_mark_a_healthy_replacement_corrupt() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "corrupt_race").await;
    tamper_row(&db, "corrupt_race", &echo_variant_wasm()).await;
    let (entered, release) = install_pause(&db, FunctionDdlHookPoint::MarkCorruptBeforeLock);

    let export_db = db.clone();
    let export = tokio::spawn(async move { export_db.export_function("corrupt_race").await });
    wait_for_pause(&entered).await;
    db.create_function_from_wasm("corrupt_race", &echo_wasm(), true)
        .await
        .expect("healthy replacement wins before stale corruption handler");
    finish_pause(&release).await;
    let error = export
        .await
        .expect("export task")
        .expect_err("original tampered row rejected");
    assert!(
        error.to_string().contains("changed during operation"),
        "got: {error}"
    );
    assert_eq!(
        db.describe_function("corrupt_race")
            .await
            .expect("describe")
            .state,
        FunctionState::Loaded
    );
    let row = stored_row(&db, "corrupt_race").await;
    assert_row_matches_binding(&db, "corrupt_race", &row);
    assert!(
        db.function_ddl_locks.is_empty(),
        "corruption check and replacement release their lock leases"
    );
}

#[tokio::test]
async fn two_concurrent_replacements_leave_catalogue_and_registry_consistent() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "replace_race").await;
    let (entered, release) = install_pause(
        &db,
        FunctionDdlHookPoint::ReplaceAfterRowSaveBeforeRegistryWrite,
    );
    let mut waits = install_wait_notice(&db);

    let first_db = db.clone();
    let first = tokio::spawn(async move {
        first_db
            .create_function_from_wasm("replace_race", &echo_variant_wasm(), true)
            .await
    });
    wait_for_pause(&entered).await;
    let second_db = db.clone();
    let second = tokio::spawn(async move {
        second_db
            .create_function_from_wasm("replace_race", &echo_wasm(), true)
            .await
    });
    wait_for_lock_wait(&mut waits, "replace_race").await;
    finish_pause(&release).await;
    first
        .await
        .expect("first replace task")
        .expect("first replace");
    second
        .await
        .expect("second replace task")
        .expect("second replace");

    let row = stored_row(&db, "replace_race").await;
    assert_row_matches_binding(&db, "replace_race", &row);
    assert_eq!(
        row["module_hash"].as_str(),
        Some(
            shamir_engine::function::ModuleHash::of(&echo_wasm())
                .to_hex()
                .as_str()
        )
    );
    assert!(
        db.function_ddl_locks.is_empty(),
        "both replacement lock leases are reclaimed"
    );
}

#[tokio::test]
async fn opposing_renames_acquire_names_in_order_and_finish_without_deadlock() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "rename_a").await;
    create_echo(&db, "rename_b").await;
    let mut acquired = install_acquired_notice(&db);

    // Observe each real rename independently: both names exist, so each reaches
    // and acquires its pair before reporting the occupied destination.
    let left_db = db.clone();
    let left = tokio::spawn(async move { left_db.rename_function("rename_a", "rename_b").await });
    let left_id = left.id();
    assert!(left.await.expect("left isolated rename task").is_err());
    assert_acquired_names(&mut acquired, left_id, &["rename_a", "rename_b"]).await;
    assert!(
        matches!(acquired.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "isolated A-to-B rename acquired exactly its two names"
    );

    let right_db = db.clone();
    let right = tokio::spawn(async move { right_db.rename_function("rename_b", "rename_a").await });
    let right_id = right.id();
    assert!(right.await.expect("right isolated rename task").is_err());
    assert_acquired_names(&mut acquired, right_id, &["rename_a", "rename_b"]).await;
    assert!(
        matches!(acquired.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "isolated B-to-A rename acquired exactly its two names"
    );

    // Separately force both opposing operations to contend before releasing the
    // first key; pending notices only stage this race, not acquisition order.
    let held = db.function_ddl_locks.lock("rename_a").await;
    let mut waits = install_wait_notice(&db);
    let start = Arc::new(tokio::sync::Barrier::new(3));

    let left_db = db.clone();
    let left_start = Arc::clone(&start);
    let mut left = tokio::spawn(async move {
        left_start.wait().await;
        left_db.rename_function("rename_a", "rename_b").await
    });
    let left_id = left.id();
    let right_db = db.clone();
    let right_start = Arc::clone(&start);
    let mut right = tokio::spawn(async move {
        right_start.wait().await;
        right_db.rename_function("rename_b", "rename_a").await
    });
    let right_id = right.id();
    start.wait().await;
    wait_for_lock_wait(&mut waits, "rename_a").await;
    wait_for_lock_wait(&mut waits, "rename_a").await;
    drop(held);

    let joined = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(&mut left, &mut right)
    })
    .await;
    if joined.is_err() {
        left.abort();
        right.abort();
        let _ = left.await;
        let _ = right.await;
        panic!("opposing renames terminate within the bounded deadline");
    }
    let (left_result, right_result) = joined.expect("checked timeout result");
    let results = (
        left_result.expect("left concurrent rename task"),
        right_result.expect("right concurrent rename task"),
    );
    assert!(results.0.is_err(), "left rename sees occupied destination");
    assert!(results.1.is_err(), "right rename sees occupied destination");

    let mut concurrent_acquisitions = FunctionDdlAcquiredNamesByTask::default();
    for _ in 0..4 {
        let (name, task_id) = tokio::time::timeout(Duration::from_secs(5), acquired.recv())
            .await
            .expect("concurrent acquisition is reported within the deadline")
            .expect("acquired notice channel remains connected");
        concurrent_acquisitions
            .entry(task_id)
            .or_insert_with(Vec::new)
            .push(name);
    }
    let expected = vec!["rename_a".to_owned(), "rename_b".to_owned()];
    assert_eq!(concurrent_acquisitions.get(&left_id), Some(&expected));
    assert_eq!(concurrent_acquisitions.get(&right_id), Some(&expected));

    assert!(db
        .system_store()
        .load_function("rename_a")
        .await
        .expect("load a")
        .is_some());
    assert!(db
        .system_store()
        .load_function("rename_b")
        .await
        .expect("load b")
        .is_some());
    assert!(
        db.function_ddl_locks.is_empty(),
        "both rename lock pairs are reclaimed"
    );
}

#[tokio::test]
async fn function_ddl_lock_entries_are_reclaimed_after_success_and_cancelled_wait() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "lock_cleanup").await;
    db.drop_function("lock_cleanup")
        .await
        .expect("drop succeeds");
    assert!(
        db.function_ddl_locks.is_empty(),
        "successful create/drop leave no entry"
    );

    let held = db.function_ddl_locks.lock("cancelled_wait").await;
    let mut waits = install_wait_notice(&db);
    let waiter_db = db.clone();
    let waiter = tokio::spawn(async move { waiter_db.drop_function("cancelled_wait").await });
    wait_for_lock_wait(&mut waits, "cancelled_wait").await;
    waiter.abort();
    let cancelled = waiter.await.expect_err("waiting DDL future was cancelled");
    assert!(cancelled.is_cancelled());
    assert!(
        !db.function_ddl_locks.is_empty(),
        "holder keeps shared entry alive"
    );
    drop(held);
    assert!(
        db.function_ddl_locks.is_empty(),
        "cancelled waiter and holder release the entry"
    );

    let missing = db.drop_function("missing_after_cancel").await;
    assert!(matches!(missing, Ok(false) | Err(DbError::NotFound(_))));
    assert!(
        db.function_ddl_locks.is_empty(),
        "failed DDL also releases its lock lease"
    );
}
