//! Review finding 3: ordinary validator registration replace and rename
//! take the target admission of every database that binds the validator,
//! with the same shared reservation and stable refusal as bind/unbind,
//! while unbound validators stay unfenced. Also covers the database-rename
//! re-key of the validator `bound_in` tracking the admission discovery
//! reads.

use std::{future::Future, path::Path, time::Duration};

use base64::Engine as _;
use shamir_query_builder::{batch::Batch, ddl};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::WriteOp;
use shamir_test_diag::FlakeGuard;
use shamir_types::types::record_id::RecordId;

use super::helpers::{on_big_stack, persistent_db, run_admin_op};
use crate::engine::repo::{repo_types::BoxRepoFactory, RepoConfig};
use crate::query::batch::BatchError;
use crate::types::value::QueryValue;
use crate::ShamirDb;

const SERVING: &str = "serving";
const TARGET: &str = "target";
const ALT: &str = "alt";
const ALT2: &str = "alt2";
const REPO: &str = "main";
const TABLE: &str = "sentinel";
const BOUND: &str = "bound_guard";
const ALTBOUND: &str = "alt_bound_guard";
const FREE: &str = "free_guard";
const FREE2: &str = "free_guard2";
const HMAC: &str = "test-hmac";
const IN_PROGRESS: &str = "schema_migration_in_progress";
const IN_DOUBT: &str = "schema_migration_in_doubt";
const DEADLINE: Duration = Duration::from_secs(180);

/// Cancel-safe: NO — on timeout the test panics; it does not reuse the engine.
async fn bounded<T>(step: impl Future<Output = T>) -> T {
    tokio::time::timeout(DEADLINE, Box::pin(step))
        .await
        .expect("validator target-fence step timed out")
}

async fn admin(db: &ShamirDb, serving: &str, op: BatchOp) -> QueryValue {
    bounded(run_admin_op(db, serving, "admin", op))
        .await
        .expect("admin op must succeed")
}

async fn refused(db: &ShamirDb, serving: &str, op: BatchOp, code: &str, case: &str) {
    let error: BatchError = bounded(run_admin_op(db, serving, "admin", op))
        .await
        .expect_err(&format!("{case}: unexpectedly admitted"));
    assert_eq!(error.code(), Some(code), "{case}: {error}");
}

fn wasm(memory_pages: u32) -> String {
    // The same minimal uploaded-module ABI as the validator admission
    // fixture; the page count distinguishes replacement artifacts so a
    // successful replace is observable through the persisted module hash.
    base64::engine::general_purpose::STANDARD.encode(
        wat::parse_str(format!(
            r#"(module
        (memory (export "memory") {memory_pages})
        (func (export "shamir_alloc") (param i32) (result i32) i32.const 1024)
        (func (export "shamir_call") (param i32 i32) (result i64) i64.const 0)
    )"#
        ))
        .unwrap(),
    )
}

fn create_validator(name: &str) -> BatchOp {
    ddl::create_validator(name).wasm(wasm(2)).build().unwrap()
}

fn replace_validator(name: &str) -> BatchOp {
    ddl::create_validator(name)
        .wasm(wasm(3))
        .replace()
        .hmac(HMAC)
        .build()
        .unwrap()
}

async fn module_hash(db: &ShamirDb, name: &str) -> String {
    bounded(db.system_store().load_validator(name))
        .await
        .unwrap()
        .expect("validator row")
        .get("module_hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn rename_validator(from: &str, to: &str) -> BatchOp {
    ddl::rename_validator(from, to)
}

async fn create_with_table(db: &ShamirDb, root: &Path, name: &str, suffix: &str) {
    bounded(db.create_db(name)).await.unwrap();
    let path = root.join(format!("{name}-{suffix}"));
    bounded(db.add_repo(name, RepoConfig::new(REPO, BoxRepoFactory::fjall(&path))))
        .await
        .unwrap();
    let mut batch = Batch::new();
    batch.op("table", ddl::create_table(TABLE).repo(REPO).build());
    let request = batch.to_request_via_msgpack().unwrap();
    bounded(db.execute(name, &request)).await.unwrap();
}

async fn bind(db: &ShamirDb, target: &str, validator: &str) {
    admin(
        db,
        SERVING,
        ddl::bind_validator(validator, TABLE)
            .db(target)
            .repo(REPO)
            .ops([WriteOp::Insert])
            .build(),
    )
    .await;
}

async fn assert_bound_tables(db: &ShamirDb, id: RecordId, mut expected: Vec<String>) {
    expected.sort();
    let mut actual = db.validators().bound_tables(&id);
    actual.sort();
    assert_eq!(actual, expected, "bound table refs");
}

#[test]
fn bound_validator_registration_refuses_fenced_binding_holders() {
    on_big_stack(|| async {
        let watchdog = FlakeGuard::with_watchdog(
            "bound_validator_registration_refuses_fenced_binding_holders",
            Duration::from_secs(600),
        );
        let (dir, db) = bounded(persistent_db()).await;
        create_with_table(&db, dir.path(), SERVING, "fixture").await;
        create_with_table(&db, dir.path(), TARGET, "fixture").await;
        create_with_table(&db, dir.path(), ALT, "fixture").await;

        watchdog.mark("fixture validators and bindings");
        admin(&db, SERVING, create_validator(BOUND)).await;
        admin(&db, SERVING, create_validator(ALTBOUND)).await;
        admin(&db, SERVING, create_validator(FREE)).await;
        let bound_id = db.validators().id_for_name(BOUND).unwrap();
        let altbound_id = db.validators().id_for_name(ALTBOUND).unwrap();
        let free_id = db.validators().id_for_name(FREE).unwrap();
        bind(&db, TARGET, BOUND).await;
        bind(&db, ALT, BOUND).await;
        bind(&db, ALT, ALTBOUND).await;
        assert_bound_tables(
            &db,
            bound_id,
            vec![
                format!("{ALT}/{REPO}/{TABLE}"),
                format!("{TARGET}/{REPO}/{TABLE}"),
            ],
        )
        .await;

        watchdog.mark("reserved fence refuses replace and rename");
        let mut gate = db.reserve_schema_migration_gate(TARGET).unwrap();
        let bound_hash_before = module_hash(&db, BOUND).await;
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_PROGRESS,
            "replace under reserved fence",
        )
        .await;
        assert_eq!(db.validators().id_for_name(BOUND), Some(bound_id));
        assert_bound_tables(
            &db,
            bound_id,
            vec![
                format!("{ALT}/{REPO}/{TABLE}"),
                format!("{TARGET}/{REPO}/{TABLE}"),
            ],
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(BOUND, "moved"),
            IN_PROGRESS,
            "rename under reserved fence",
        )
        .await;
        assert_eq!(db.validators().id_for_name(BOUND), Some(bound_id));
        assert_eq!(
            module_hash(&db, BOUND).await,
            bound_hash_before,
            "a refused replace must not swap the artifact"
        );

        watchdog.mark("unfenced validators stay unfenced");
        let free_hash_before = module_hash(&db, FREE).await;
        let replaced = admin(&db, SERVING, replace_validator(FREE)).await;
        assert_eq!(replaced["created_validator"].as_str(), Some(FREE));
        assert_eq!(db.validators().id_for_name(FREE), Some(free_id));
        assert_ne!(
            module_hash(&db, FREE).await,
            free_hash_before,
            "the replacement artifact must actually swap the module"
        );
        let renamed = admin(&db, SERVING, rename_validator(FREE, FREE2)).await;
        assert_eq!(renamed["renamed_validator"].as_str(), Some(FREE));
        assert_eq!(db.validators().id_for_name(FREE), None);
        assert_eq!(db.validators().id_for_name(FREE2), Some(free_id));
        let row = bounded(db.system_store().load_validator(FREE2))
            .await
            .unwrap()
            .expect("renamed validator keeps a durable row");
        assert_eq!(row["_id"].as_str(), Some(free_id.to_string().as_str()));
        assert!(
            bounded(db.system_store().load_validator(FREE))
                .await
                .unwrap()
                .is_none(),
            "old durable row must be re-keyed"
        );

        watchdog.mark("in-doubt fence refuses replace and rename");
        gate.hold(1);
        drop(gate);
        assert_eq!(
            db.schema_migration_gate_in_doubt_generation(TARGET),
            Some(1)
        );
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_DOUBT,
            "replace under in-doubt fence",
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(BOUND, "moved"),
            IN_DOUBT,
            "rename under in-doubt fence",
        )
        .await;

        watchdog.mark("database rename re-keys the bound tracking");
        admin(&db, SERVING, ddl::rename_db(ALT, ALT2).build()).await;
        assert_eq!(db.validators().id_for_name(BOUND), Some(bound_id));
        assert_bound_tables(
            &db,
            bound_id,
            vec![
                format!("{ALT2}/{REPO}/{TABLE}"),
                format!("{TARGET}/{REPO}/{TABLE}"),
            ],
        )
        .await;
        let row = bounded(db.system_store().load_validator(BOUND))
            .await
            .unwrap()
            .expect("bound validator keeps its durable row");
        let bound_in: Vec<String> = row["bound_in"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect();
        assert!(bound_in.contains(&format!("{ALT2}/{REPO}/{TABLE}")));
        assert!(!bound_in.contains(&format!("{ALT}/{REPO}/{TABLE}")));

        watchdog.mark("replace with only free bindings succeeds and keeps them");
        let altbound_hash_before = module_hash(&db, ALTBOUND).await;
        let replaced = admin(&db, SERVING, replace_validator(ALTBOUND)).await;
        assert_eq!(replaced["created_validator"].as_str(), Some(ALTBOUND));
        assert_eq!(db.validators().id_for_name(ALTBOUND), Some(altbound_id));
        assert_ne!(
            module_hash(&db, ALTBOUND).await,
            altbound_hash_before,
            "the free-only replace must actually swap the module"
        );
        assert_bound_tables(&db, altbound_id, vec![format!("{ALT2}/{REPO}/{TABLE}")]).await;

        watchdog.mark("admission follows the re-keyed name");
        let mut gate2 = db.reserve_schema_migration_gate(ALT2).unwrap();
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_PROGRESS,
            "replace after rename, reserved",
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(ALTBOUND, "moved"),
            IN_PROGRESS,
            "rename after rename, reserved",
        )
        .await;
        gate2.hold(2);
        drop(gate2);
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_DOUBT,
            "replace after rename, in doubt",
        )
        .await;
        assert_eq!(db.validators().id_for_name(BOUND), Some(bound_id));
        assert_eq!(
            module_hash(&db, BOUND).await,
            bound_hash_before,
            "the in-doubt refusal must not swap the artifact"
        );
    });
}
