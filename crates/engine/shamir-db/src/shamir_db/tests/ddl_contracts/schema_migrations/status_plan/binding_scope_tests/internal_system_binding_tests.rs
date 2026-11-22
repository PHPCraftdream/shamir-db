//! Internal system-store validator binding regression; user `__system__`
//! lifecycle cases live with the reserved-name tests.

use std::time::Duration;

use shamir_engine::validator::{ValidatorBinding, WriteOp as BindingWriteOp};
use shamir_test_diag::FlakeGuard;

use super::super::helpers::{init_waiting_for_locks, on_big_stack, persistent_db};
use super::{
    admin, create_validator, drop_validator_op, module_hash, refused_still_bound,
    replace_validator, validator_row, REPO, SERVING, SETTINGS_TABLE,
};
use crate::engine::table::TableManager;
use crate::ShamirDb;

const DEADLINE: Duration = Duration::from_secs(180);
const INTERNAL: &str = "internal_ref_guard";
const INTERNAL_REF: &str = "__system__/system/settings";
const INTERNAL_PRIORITY: u16 = 1000;

async fn bounded<T>(step: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(DEADLINE, Box::pin(step))
        .await
        .expect("internal validator binding step timed out")
}

async fn reopen(dir: &std::path::Path, db: ShamirDb) -> ShamirDb {
    drop(db);
    init_waiting_for_locks(dir.join("system")).await
}

#[test]
fn internal_system_binding_is_visible_and_removable() {
    on_big_stack(|| async {
        let watchdog = FlakeGuard::with_watchdog(
            "internal_system_binding_is_visible_and_removable",
            Duration::from_secs(600),
        );
        let (dir, db) = bounded(persistent_db()).await;
        bounded(db.create_db(SERVING)).await.unwrap();
        let repo_path = dir.path().join("serving-repo");
        bounded(db.add_repo(
            SERVING,
            crate::engine::repo::RepoConfig::new(
                REPO,
                crate::engine::repo::repo_types::BoxRepoFactory::fjall(&repo_path),
            ),
        ))
        .await
        .unwrap();
        let mut tables = shamir_query_builder::batch::Batch::new();
        tables.op(
            "table",
            shamir_query_builder::ddl::create_table("sentinel")
                .repo(REPO)
                .build(),
        );
        let request = tables.to_request_via_msgpack().unwrap();
        bounded(db.execute(SERVING, &request)).await.unwrap();

        watchdog.mark("persist an actual internal settings-table binding");
        admin(&db, SERVING, create_validator(INTERNAL)).await;
        let id = db.validators().id_for_name(INTERNAL).unwrap();
        let settings: TableManager = db
            .system_store()
            .system_repo()
            .expect("system repo")
            .get_table(SETTINGS_TABLE)
            .await
            .expect("settings table");
        settings
            .add_validator_binding(ValidatorBinding {
                validator_id: id,
                ops: vec![BindingWriteOp::Insert].into(),
                priority: INTERNAL_PRIORITY,
            })
            .await
            .expect("internal binding write succeeds");
        db.system_store()
            .flush_system_repo_durable(&settings)
            .await
            .expect("the settings data view is flushed");
        drop(settings);

        watchdog.mark("restart preserves the internal binding envelope");
        let db = reopen(dir.path(), db).await;
        assert_eq!(db.validators().id_for_name(INTERNAL), Some(id));
        let persisted = db
            .system_store()
            .system_repo()
            .expect("system repo")
            .table_validator_bindings_metadata(SETTINGS_TABLE)
            .await
            .expect("existing-only metadata read")
            .expect("settings has persisted validator metadata");
        assert!(persisted
            .bindings
            .iter()
            .any(|binding| binding.validator_id == id));

        watchdog.mark("internal binding is exempt from user-target reservations");
        let before = module_hash(&db, INTERNAL).await;
        admin(&db, SERVING, replace_validator(INTERNAL)).await;
        assert_ne!(module_hash(&db, INTERNAL).await, before);

        watchdog.mark("internal binding still blocks DROP with its table identity");
        refused_still_bound(
            &db,
            SERVING,
            drop_validator_op(INTERNAL),
            INTERNAL_REF,
            "drop of internally bound validator",
        )
        .await;

        watchdog.mark("remove through the actual internal TableManager then DROP succeeds");
        let settings = db
            .system_store()
            .system_repo()
            .expect("system repo")
            .get_table(SETTINGS_TABLE)
            .await
            .expect("settings table");
        assert!(settings.remove_validator_binding(&id).await.unwrap());
        admin(&db, SERVING, drop_validator_op(INTERNAL)).await;
        assert_eq!(db.validators().id_for_name(INTERNAL), None);
        assert!(validator_row(&db, INTERNAL).await.is_none());
        drop(settings);

        watchdog.mark("normal configured repo survives another restart");
        let reopened = reopen(dir.path(), db).await;
        assert!(reopened.has_db(SERVING));
        assert!(reopened.get_db(SERVING).unwrap().get_repo(REPO).is_some());
    });
}
