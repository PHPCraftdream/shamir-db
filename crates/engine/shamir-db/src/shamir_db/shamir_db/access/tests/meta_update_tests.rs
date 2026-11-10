use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use tokio::sync::{mpsc, Notify};

use crate::access::{Action, Actor, ResourceMeta, ResourcePath};
use crate::query::batch::BatchError;
use crate::shamir_db::shamir_db::core::FunctionDdlHookPoint;
use crate::shamir_db::ShamirDb;

#[tokio::test]
async fn function_chmod_does_not_restore_owner_from_concurrent_chown() {
    let shamir = ShamirDb::init_memory().await.expect("initialize database");
    shamir.create_db("testdb").await.expect("create database");
    let wasm = wat::parse_str(
        r#"(module
          (memory (export "memory") 2)
          (global $bump (mut i32) (i32.const 1024))
          (func (export "shamir_alloc") (param $len i32) (result i32)
            (local $ptr i32)
            (local.set $ptr (global.get $bump))
            (global.set $bump (i32.add (global.get $bump) (local.get $len)))
            (local.get $ptr))
          (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
            (i64.or
              (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
              (i64.extend_i32_u (local.get $len)))))"#,
    )
    .expect("valid function fixture");
    shamir
        .create_function_from_wasm("meta_race", &wasm, false)
        .await
        .expect("create function");
    shamir
        .set_resource_meta(
            &ResourcePath::function("meta_race"),
            &ResourceMeta {
                owner: Actor::User(41),
                group: Some(7),
                mode: 0o700,
            },
        )
        .await
        .expect("set initial metadata");

    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let hook_entered = Arc::clone(&entered);
    let hook_release = Arc::clone(&release);
    let fired = Arc::new(AtomicBool::new(false));
    let hook_fired = Arc::clone(&fired);
    assert!(shamir
        .function_ddl_hook
        .set(Box::new(move |point| {
            let entered = Arc::clone(&hook_entered);
            let release = Arc::clone(&hook_release);
            let fired = Arc::clone(&hook_fired);
            Box::pin(async move {
                if point == FunctionDdlHookPoint::MetaAfterRowReadBeforeWrite
                    && !fired.swap(true, Ordering::SeqCst)
                {
                    entered.notify_one();
                    release.notified().await;
                }
            })
        }))
        .is_ok());
    let (wait_tx, mut wait_rx) = mpsc::channel(4);
    shamir
        .function_ddl_locks
        .set_wait_hook(Arc::new(move |name| {
            wait_tx
                .try_send(name)
                .expect("test wait channel has capacity");
        }));

    let mut chown_batch = Batch::new();
    chown_batch.id(1);
    chown_batch.chown(
        "chown",
        ddl::chown(ddl::res::function("meta_race"), 42)
            .hmac("test")
            .build()
            .expect("build chown"),
    );
    let chown_request = chown_batch
        .to_request_via_msgpack()
        .expect("encode chown request");
    let chown_db = shamir.clone();
    let chown = tokio::spawn(async move {
        chown_db
            .execute_as(Actor::System, "testdb", &chown_request)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("chown reached its locked metadata read");

    let mut chmod_batch = Batch::new();
    chmod_batch.id(2);
    chmod_batch.chmod(
        "chmod",
        ddl::chmod(ddl::res::function("meta_race"), 0o750)
            .hmac("test")
            .build()
            .expect("build chmod"),
    );
    let chmod_request = chmod_batch
        .to_request_via_msgpack()
        .expect("encode chmod request");
    let chmod_db = shamir.clone();
    let chmod = tokio::spawn(async move {
        chmod_db
            .execute_as(Actor::System, "testdb", &chmod_request)
            .await
    });
    let waiting_name = tokio::time::timeout(Duration::from_secs(5), wait_rx.recv())
        .await
        .expect("chmod reached the function lock")
        .expect("wait hook remains connected");
    assert_eq!(waiting_name, "meta_race");

    release.notify_one();
    chown.await.expect("chown task").expect("chown succeeds");
    chmod.await.expect("chmod task").expect("chmod succeeds");

    let actual = shamir
        .resource_meta(&ResourcePath::function("meta_race"))
        .await
        .expect("load updated metadata");
    assert_eq!(actual.owner, Actor::User(42));
    assert_eq!(actual.group, Some(7));
    assert_eq!(actual.mode, 0o750);
    assert!(
        shamir.function_ddl_locks.is_empty(),
        "metadata updates release their function lock lease"
    );
}

async fn setup_user_owned_function(name: &str) -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.expect("initialize database");
    shamir.create_db("testdb").await.expect("create database");
    shamir
        .set_resource_meta(
            &ResourcePath::database("testdb"),
            &ResourceMeta {
                owner: Actor::System,
                group: None,
                mode: 0o755,
            },
        )
        .await
        .expect("allow user database traversal");
    let wasm = wat::parse_str(
        r#"(module
          (memory (export "memory") 2)
          (global $bump (mut i32) (i32.const 1024))
          (func (export "shamir_alloc") (param $len i32) (result i32)
            (local $ptr i32)
            (local.set $ptr (global.get $bump))
            (global.set $bump (i32.add (global.get $bump) (local.get $len)))
            (local.get $ptr))
          (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
            (i64.or
              (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
              (i64.extend_i32_u (local.get $len)))))"#,
    )
    .expect("valid function fixture");
    shamir
        .create_function_from_wasm(name, &wasm, false)
        .await
        .expect("create function");
    shamir
        .set_resource_meta(
            &ResourcePath::function(name),
            &ResourceMeta {
                owner: Actor::User(41),
                group: None,
                mode: 0o700,
            },
        )
        .await
        .expect("set initial function metadata");
    shamir
}

fn install_meta_update_pause(shamir: &ShamirDb) -> (Arc<Notify>, Arc<Notify>) {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let hook_entered = Arc::clone(&entered);
    let hook_release = Arc::clone(&release);
    let fired = Arc::new(AtomicBool::new(false));
    let hook_fired = Arc::clone(&fired);
    assert!(shamir
        .function_ddl_hook
        .set(Box::new(move |point| {
            let entered = Arc::clone(&hook_entered);
            let release = Arc::clone(&hook_release);
            let fired = Arc::clone(&hook_fired);
            Box::pin(async move {
                if point == FunctionDdlHookPoint::MetaAfterRowReadBeforeWrite
                    && !fired.swap(true, Ordering::SeqCst)
                {
                    entered.notify_one();
                    release.notified().await;
                }
            })
        }))
        .is_ok());
    (entered, release)
}

fn install_function_lock_wait_notice(shamir: &ShamirDb) -> mpsc::Receiver<String> {
    let (wait_tx, wait_rx) = mpsc::channel(4);
    shamir
        .function_ddl_locks
        .set_wait_hook(Arc::new(move |name| {
            wait_tx
                .try_send(name)
                .expect("test wait channel has capacity");
        }));
    wait_rx
}

async fn wait_for_function_pause(entered: &Notify) {
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("first metadata update reached its locked row read");
}

async fn wait_for_function_lock_wait(receiver: &mut mpsc::Receiver<String>, name: &str) {
    let waiting_name = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("competing request reached the function lock")
        .expect("function lock wait channel remains connected");
    assert_eq!(waiting_name, name);
}

fn assert_access_denied(error: BatchError, expected_message: &str) {
    assert_eq!(error.code(), Some("access_denied"));
    match error {
        BatchError::QueryError { message, .. } => assert_eq!(message, expected_message),
        other => panic!("expected access_denied QueryError, got {other:?}"),
    }
}

#[tokio::test]
async fn function_chmod_rechecks_manage_after_concurrent_admin_chown() {
    let name = "admin_chown_chmod_race";
    let shamir = setup_user_owned_function(name).await;
    let (entered, release) = install_meta_update_pause(&shamir);
    let mut wait_rx = install_function_lock_wait_notice(&shamir);

    let mut chown_batch = Batch::new();
    chown_batch.id(1);
    chown_batch.chown(
        "chown",
        ddl::chown(ddl::res::function(name), 42)
            .hmac("test")
            .build()
            .expect("build chown"),
    );
    let chown_request = chown_batch
        .to_request_via_msgpack()
        .expect("encode chown request");
    let chown_db = shamir.clone();
    let chown = tokio::spawn(async move {
        chown_db
            .execute_as(Actor::Admin(99), "testdb", &chown_request)
            .await
    });
    wait_for_function_pause(&entered).await;

    let function_path = ResourcePath::function(name);
    shamir
        .authorize_access(&Actor::User(41), &function_path, Action::Manage)
        .await
        .expect("owner 41 still passes preauthorization before chown commits");
    let mut chmod_batch = Batch::new();
    chmod_batch.id(2);
    chmod_batch.chmod(
        "chmod",
        ddl::chmod(ddl::res::function(name), 0o750)
            .hmac("test")
            .build()
            .expect("build chmod"),
    );
    let chmod_request = chmod_batch
        .to_request_via_msgpack()
        .expect("encode chmod request");
    let chmod_db = shamir.clone();
    let chmod = tokio::spawn(async move {
        chmod_db
            .execute_as(Actor::User(41), "testdb", &chmod_request)
            .await
    });
    wait_for_function_lock_wait(&mut wait_rx, name).await;

    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), chown)
        .await
        .expect("administrator chown completes within the deadline")
        .expect("chown task")
        .expect("administrator chown succeeds");
    let chmod_error = tokio::time::timeout(Duration::from_secs(5), chmod)
        .await
        .expect("chmod completes within the deadline")
        .expect("chmod task")
        .expect_err("former owner chmod is denied after waiting for chown");
    let expected_message = shamir
        .authorize_access(&Actor::User(41), &function_path, Action::Manage)
        .await
        .expect_err("owner 41 no longer has Manage after chown")
        .to_string();
    assert_access_denied(chmod_error, &expected_message);

    let actual = shamir
        .resource_meta(&function_path)
        .await
        .expect("load updated metadata");
    assert_eq!(actual.owner, Actor::User(42));
    assert_eq!(actual.group, None);
    assert_eq!(actual.mode, 0o700);
    assert!(
        shamir.function_ddl_locks.is_empty(),
        "metadata updates release their function lock lease"
    );
}

#[tokio::test]
async fn function_chown_rechecks_manage_after_concurrent_owner_chown() {
    let name = "owner_chown_chown_race";
    let shamir = setup_user_owned_function(name).await;
    let (entered, release) = install_meta_update_pause(&shamir);
    let mut wait_rx = install_function_lock_wait_notice(&shamir);

    let mut first_batch = Batch::new();
    first_batch.id(1);
    first_batch.chown(
        "first_chown",
        ddl::chown(ddl::res::function(name), 42)
            .hmac("test")
            .build()
            .expect("build first chown"),
    );
    let first_request = first_batch
        .to_request_via_msgpack()
        .expect("encode first chown request");
    let first_db = shamir.clone();
    let first_chown = tokio::spawn(async move {
        first_db
            .execute_as(Actor::User(41), "testdb", &first_request)
            .await
    });
    wait_for_function_pause(&entered).await;

    let function_path = ResourcePath::function(name);
    shamir
        .authorize_access(&Actor::User(41), &function_path, Action::Manage)
        .await
        .expect("owner 41 still passes preauthorization before first chown commits");
    let mut second_batch = Batch::new();
    second_batch.id(2);
    second_batch.chown(
        "second_chown",
        ddl::chown(ddl::res::function(name), 43)
            .hmac("test")
            .build()
            .expect("build second chown"),
    );
    let second_request = second_batch
        .to_request_via_msgpack()
        .expect("encode second chown request");
    let second_db = shamir.clone();
    let second_chown = tokio::spawn(async move {
        second_db
            .execute_as(Actor::User(41), "testdb", &second_request)
            .await
    });
    wait_for_function_lock_wait(&mut wait_rx, name).await;

    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), first_chown)
        .await
        .expect("first chown completes within the deadline")
        .expect("first chown task")
        .expect("first owner chown succeeds");
    let second_error = tokio::time::timeout(Duration::from_secs(5), second_chown)
        .await
        .expect("second chown completes within the deadline")
        .expect("second chown task")
        .expect_err("former owner chown is denied after waiting for first chown");
    let expected_message = shamir
        .authorize_access(&Actor::User(41), &function_path, Action::Manage)
        .await
        .expect_err("owner 41 no longer has Manage after first chown")
        .to_string();
    assert_access_denied(second_error, &expected_message);

    let actual = shamir
        .resource_meta(&function_path)
        .await
        .expect("load updated metadata");
    assert_eq!(actual.owner, Actor::User(42));
    assert_eq!(actual.group, None);
    assert_eq!(actual.mode, 0o700);
    assert!(
        shamir.function_ddl_locks.is_empty(),
        "metadata updates release their function lock lease"
    );
}
