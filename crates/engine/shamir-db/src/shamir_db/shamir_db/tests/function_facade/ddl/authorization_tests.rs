use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;
use tokio::sync::{mpsc, Notify};

use super::super::support::{create_echo, stored_row};
use crate::access::{Action, Actor, ResourceMeta, ResourcePath};
use crate::query::batch::{BatchError, BatchRequest};
use crate::shamir_db::shamir_db::core::FunctionDdlHookPoint;
use crate::shamir_db::ShamirDb;

#[derive(Clone, Copy)]
enum DdlAction {
    Drop,
    Alter,
    Rename,
}

impl DdlAction {
    fn access_action(self) -> Action {
        match self {
            Self::Drop => Action::Delete,
            Self::Alter | Self::Rename => Action::Write,
        }
    }
}

fn function_request(name: &str, action: DdlAction, id: i64) -> BatchRequest {
    let mut batch = Batch::new();
    batch.id(id);
    match action {
        DdlAction::Drop => batch.drop_function(
            "ddl",
            ddl::drop_function(name)
                .hmac("test")
                .build()
                .expect("build drop function"),
        ),
        DdlAction::Alter => batch.alter_function("ddl", ddl::alter_function(name, "never")),
        DdlAction::Rename => {
            batch.rename_function("ddl", ddl::rename_function(name, format!("{name}_renamed")))
        }
    };
    batch
        .to_request_via_msgpack()
        .expect("encode function DDL request")
}

fn chown_request(name: &str) -> BatchRequest {
    let mut batch = Batch::new();
    batch.id(100);
    batch.chown(
        "chown",
        ddl::chown(ddl::res::function(name), 42)
            .hmac("test")
            .build()
            .expect("build chown"),
    );
    batch
        .to_request_via_msgpack()
        .expect("encode chown request")
}

async fn setup_user_owned_function(name: &str) -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("initialize database");
    db.create_db("testdb").await.expect("create test database");
    db.set_resource_meta(
        &ResourcePath::database("testdb"),
        &ResourceMeta {
            owner: Actor::System,
            group: None,
            mode: 0o755,
        },
    )
    .await
    .expect("allow user database reads");
    create_echo(&db, name).await;
    db.set_resource_meta(
        &ResourcePath::function(name),
        &ResourceMeta {
            owner: Actor::User(41),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("set initial function metadata");
    db
}

fn install_meta_update_pause(db: &ShamirDb) -> (Arc<Notify>, Arc<Notify>) {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let hook_entered = Arc::clone(&entered);
    let hook_release = Arc::clone(&release);
    let fired = Arc::new(AtomicBool::new(false));
    let hook_fired = Arc::clone(&fired);
    assert!(db
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

fn install_function_lock_wait_notice(db: &ShamirDb) -> mpsc::Receiver<String> {
    let (wait_tx, wait_rx) = mpsc::channel(4);
    db.function_ddl_locks.set_wait_hook(Arc::new(move |name| {
        wait_tx
            .try_send(name)
            .expect("test wait channel has capacity");
    }));
    wait_rx
}

async fn wait_for_function_pause(entered: &Notify) {
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("admin chown reached its locked row read");
}

async fn wait_for_function_lock_wait(receiver: &mut mpsc::Receiver<String>, name: &str) {
    let waiting_name = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("competing request reached the function lock")
        .expect("function lock wait channel remains connected");
    assert_eq!(waiting_name, name);
}

fn artifact_id(row: &QueryValue) -> &str {
    row.get("artifact_id")
        .and_then(QueryValue::as_str)
        .expect("row artifact id")
}

fn assert_row_matches_binding(db: &ShamirDb, name: &str, row: &QueryValue) {
    let binding = db
        .functions()
        .capture(name)
        .expect("registered function binding");
    let binding_id = binding
        .artifact_id()
        .expect("binding artifact id")
        .to_owned();
    assert_eq!(artifact_id(row), binding_id);
}

fn assert_access_denied(error: BatchError, expected_message: &str) {
    assert_eq!(error.code(), Some("access_denied"));
    match error {
        BatchError::QueryError { message, .. } => assert_eq!(message, expected_message),
        other => panic!("expected access_denied QueryError, got {other:?}"),
    }
}

async fn former_owner_ddl_is_denied_after_admin_chown(action: DdlAction, name: &str) {
    let db = setup_user_owned_function(name).await;
    let original_row = stored_row(&db, name).await;
    let original_binding = db
        .functions()
        .capture(name)
        .expect("initial function binding");
    let original_binding_id = original_binding
        .artifact_id()
        .expect("initial artifact id")
        .to_owned();
    let (entered, release) = install_meta_update_pause(&db);
    let mut wait_rx = install_function_lock_wait_notice(&db);

    let admin_db = db.clone();
    let request = chown_request(name);
    let chown = tokio::spawn(async move {
        admin_db
            .execute_as(Actor::Admin(99), "testdb", &request)
            .await
    });
    wait_for_function_pause(&entered).await;

    let path = ResourcePath::function(name);
    db.authorize_access(&Actor::User(41), &path, action.access_action())
        .await
        .expect("former owner is authorized against the pre-chown row");
    let request = function_request(name, action, 101);
    let contender_db = db.clone();
    let contender = tokio::spawn(async move {
        contender_db
            .execute_as(Actor::User(41), "testdb", &request)
            .await
    });
    wait_for_function_lock_wait(&mut wait_rx, name).await;

    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), chown)
        .await
        .expect("administrator chown completes")
        .expect("chown task")
        .expect("administrator chown succeeds");
    let error = tokio::time::timeout(Duration::from_secs(5), contender)
        .await
        .expect("former owner's DDL completes")
        .expect("former-owner DDL task")
        .expect_err("former owner must be denied after waiting for chown");
    let expected_message = db
        .authorize_access(&Actor::User(41), &path, action.access_action())
        .await
        .expect_err("former owner no longer has the required permission")
        .to_string();
    assert_access_denied(error, &expected_message);

    let mut expected_row = original_row;
    let mut new_meta = ResourceMeta::from_record(&expected_row);
    new_meta.owner = Actor::User(42);
    new_meta.inject_into(&mut expected_row);
    let row_after = stored_row(&db, name).await;
    assert_eq!(
        row_after, expected_row,
        "denied DDL leaves the chowned row unchanged"
    );
    let binding_after = db
        .functions()
        .capture(name)
        .expect("function binding remains registered");
    assert!(
        original_binding.same_as(&binding_after),
        "denied DDL leaves the original function binding unchanged"
    );
    let meta_after = ResourceMeta::from_record(&row_after);
    assert_eq!(meta_after.owner, Actor::User(42));
    assert_eq!(meta_after.group, None);
    assert_eq!(meta_after.mode, 0o700);
    assert_eq!(artifact_id(&row_after), original_binding_id);
    assert_row_matches_binding(&db, name, &row_after);
    if matches!(action, DdlAction::Rename) {
        assert!(
            db.system_store()
                .load_function(&format!("{name}_renamed"))
                .await
                .expect("check rename target")
                .is_none(),
            "denied rename does not create a destination row"
        );
        assert!(!db.functions().contains(&format!("{name}_renamed")));
    }
}

#[tokio::test]
async fn former_owner_drop_is_denied_after_admin_chown_while_waiting() {
    former_owner_ddl_is_denied_after_admin_chown(DdlAction::Drop, "auth_drop_race").await;
}

#[tokio::test]
async fn former_owner_alter_is_denied_after_admin_chown_while_waiting() {
    former_owner_ddl_is_denied_after_admin_chown(DdlAction::Alter, "auth_alter_race").await;
}

#[tokio::test]
async fn former_owner_rename_is_denied_after_admin_chown_while_waiting() {
    former_owner_ddl_is_denied_after_admin_chown(DdlAction::Rename, "auth_rename_race").await;
}

#[tokio::test]
async fn former_owner_drop_queued_during_recreation_cannot_drop_new_owner_function() {
    let name = "auth_recreate_race";
    let db = setup_user_owned_function(name).await;
    let old_row = stored_row(&db, name).await;
    let old_artifact_id = artifact_id(&old_row).to_owned();
    let replacement_artifact = db
        .functions()
        .get(name)
        .expect("capture compiled artifact for trusted recreation");

    // This externally-held name lock represents the admin's critical section.
    // The trusted catalogue/registry replacement below completes under it,
    // avoiding a competing DDL call trying to recursively acquire the lock.
    let admin_ddl_guard = db.function_ddl_locks.lock(name).await;
    let mut wait_rx = install_function_lock_wait_notice(&db);
    let former_owner_request = function_request(name, DdlAction::Drop, 201);
    let former_owner_db = db.clone();
    let former_owner_drop = tokio::spawn(async move {
        former_owner_db
            .execute_as(Actor::User(41), "testdb", &former_owner_request)
            .await
    });
    wait_for_function_lock_wait(&mut wait_rx, name).await;

    db.system_store()
        .remove_function(name)
        .await
        .expect("remove old catalogue row during recreation");
    let new_artifact_id = RecordId::new().to_string();
    let new_meta = ResourceMeta {
        owner: Actor::User(42),
        group: None,
        mode: 0o700,
    };
    let mut new_row = old_row;
    if let QueryValue::Map(fields) = &mut new_row {
        fields.insert(
            "artifact_id".to_owned(),
            QueryValue::Str(new_artifact_id.clone()),
        );
    }
    new_meta.inject_into(&mut new_row);
    db.system_store()
        .save_function(name, &new_row, &new_meta)
        .await
        .expect("save recreated catalogue row");
    db.functions().replace_with_artifact_id(
        name,
        replacement_artifact,
        Some(new_artifact_id.clone()),
    );
    let recreated_row = stored_row(&db, name).await;
    assert_ne!(old_artifact_id, new_artifact_id);
    assert_eq!(artifact_id(&recreated_row), new_artifact_id);
    assert_row_matches_binding(&db, name, &recreated_row);

    drop(admin_ddl_guard);
    let error = tokio::time::timeout(Duration::from_secs(5), former_owner_drop)
        .await
        .expect("queued former-owner drop completes")
        .expect("former-owner drop task")
        .expect_err("former owner cannot drop the recreated function");
    let expected_message = db
        .authorize_access(
            &Actor::User(41),
            &ResourcePath::function(name),
            Action::Delete,
        )
        .await
        .expect_err("former owner has no Delete on the new owner's row")
        .to_string();
    assert_access_denied(error, &expected_message);

    let row_after = stored_row(&db, name).await;
    assert_eq!(
        row_after, recreated_row,
        "denied drop preserves recreated row"
    );
    assert_eq!(ResourceMeta::from_record(&row_after).owner, Actor::User(42));
    assert_eq!(ResourceMeta::from_record(&row_after).mode, 0o700);
    assert_eq!(artifact_id(&row_after), new_artifact_id);
    assert_row_matches_binding(&db, name, &row_after);
}
