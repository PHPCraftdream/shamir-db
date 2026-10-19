#![cfg(all(unix, feature = "fjall"))]

use std::sync::Arc;

use shamir_engine::meta::repl::bookmark_v3::load_bookmark_v3;
use shamir_engine::meta::repo_identity::RepoIdentity;
use shamir_engine::repo::{
    BoxRepoFactory, RepoFactory, RepoInstance, SeedImageWriter, SeedInstallState, SeedReceipt,
    SeedTable,
};
use shamir_engine::table::table_manager::table_token_for;
use shamir_engine::tx::{apply_replicated_batch, ApplyOutcome, ApplyPolicy, ReplicatedEvent};
use shamir_query_builder::doc;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{Repo, Store};
use shamir_tx::{project_event, IsolationLevel, TxContext, TxId};
use shamir_types::access::Actor as WireActor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_types::types::value::QueryValue;
use tokio::runtime::{Builder, Runtime};

use crate::access::{Actor, ResourceMeta};
use crate::{ShamirDb, SystemStoreConfig};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

fn receipt() -> SeedReceipt {
    SeedReceipt {
        artifact_digest: "a".repeat(64),
        source_db: "app".into(),
        source_repo: "main".into(),
        source_identity: RepoIdentity {
            lineage_id: [7; 16],
            history_epoch: 1,
        },
        schema_digest: "b".repeat(64),
        source_version: 73,
        local_baseline: 1,
    }
}

#[tokio::test]
async fn pre_registry_gate_distinguishes_ordinary_designated_and_installing() {
    let root = tempfile::tempdir().unwrap();
    let home = ShamirDb::init_memory().await.unwrap();
    let path = root.path().join("image");
    std::fs::create_dir(&path).unwrap();
    home.seed_boot_gate("fjall", path.to_str()).await.unwrap();
    std::fs::write(path.join(".seed-install"), b"SEED-3").unwrap();
    assert!(home.seed_boot_gate("fjall", path.to_str()).await.is_err());
    let backing = BoxRepoFactory::fjall_raw(&path).create().await.unwrap();
    let image = RepoInstance::new("main".into(), backing, Vec::new());
    let writer = SeedImageWriter::begin(image.clone(), receipt(), Vec::new())
        .await
        .unwrap();
    drop(writer);
    drop(image);
    assert!(home.seed_boot_gate("fjall", path.to_str()).await.is_err());
    assert!(home.get_db("app").is_none());
}

#[test]
fn normal_boot_refuses_missing_and_installing_receipts_before_registry() {
    for installing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let image = root.path().join("image");
        let catalogue = root.path().join("shamir_db_meta.redb");
        std::fs::create_dir(&image).unwrap();
        std::fs::write(image.join(".seed-install"), b"SEED-3").unwrap();
        let rt = runtime();
        rt.block_on(async {
            if installing {
                let backing = BoxRepoFactory::fjall_raw(&image).create().await.unwrap();
                let repo = RepoInstance::new("main".into(), backing, Vec::new());
                let writer = SeedImageWriter::begin(repo.clone(), receipt(), Vec::new())
                    .await
                    .unwrap();
                drop(writer);
                assert!(matches!(
                    SeedInstallState::load(&repo.tx_info_store().await.unwrap())
                        .await
                        .unwrap(),
                    Some(SeedInstallState::Installing(_))
                ));
            }
            let home = ShamirDb::init(SystemStoreConfig::Fjall(catalogue.clone()))
                .await
                .unwrap();
            let metadata = ResourceMeta::owned_enforced(Actor::System);
            let record: QueryValue = doc! { "name" => "app" }.into();
            home.system_store
                .save_database("app", &record, &metadata)
                .await
                .unwrap();
            home.system_store
                .save_repository("app", "main", "fjall", image.to_str(), &metadata, None)
                .await
                .unwrap();
            home.flush_seed_catalogue().await.unwrap();
        });
        drop(rt);
        let rt = runtime();
        let result = rt.block_on(ShamirDb::init(SystemStoreConfig::Fjall(catalogue)));
        let error = match result {
            Ok(_) => panic!("ordinary init served a partial image"),
            Err(error) => error,
        };
        let expected = if installing {
            "incomplete"
        } else {
            "no Complete"
        };
        assert!(error.to_string().contains(expected));
    }
}

/// Build a Put event via the REAL `shamir_tx::project_event` path, exactly
/// like a leader's journal read emits it (same shape as the engine's
/// `apply_replicated_tests::put_event` helper).
fn put_event(value: &str, record: RecordId, leader_version: u64) -> shamir_tx::ChangelogEvent {
    let mut tx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    tx.set_actor(WireActor::User(42));
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let body = InnerValue::Str(value.into()).to_bytes().unwrap();
    tx.ensure_table_staging(table_token_for("items"), "items", data_store)
        .set(record.to_bytes().into(), body);
    project_event(&tx, "leader", leader_version).unwrap()
}

/// R7-3 regression: a seed image staged on RAW Fjall and registered with the
/// catalogue kind `"fjall_raw"` reopens through the NORMAL boot path as an
/// unbuffered repo whose backend reports `supports_atomic_batch() == true`,
/// so the first replicated batch applies and the durable bookmark advances.
///
/// COUNTERFACTUAL: with catalogue kind `"fjall"` the boot path
/// (`factory_from_meta`) re-attaches the image MemBuffer-wrapped; the
/// receipt pipeline's admission check (`admit_follower`) then fails with
/// `DbError::AtomicBatchUnsupported` on EVERY attempt and the follower
/// stalls forever instead of returning `Applied` — the CI stall this test
/// pins.
#[test]
fn seeded_raw_fjall_image_reboots_atomic_batch_capable_and_applies() {
    let root = tempfile::tempdir().unwrap();
    let image = root.path().join("image");
    let catalogue = root.path().join("shamir_db_meta.redb");
    std::fs::create_dir(&image).unwrap();
    let rt = runtime();
    rt.block_on(async {
        let home = ShamirDb::init(SystemStoreConfig::Fjall(catalogue.clone()))
            .await
            .unwrap();
        // Stage the image on RAW Fjall exactly like production
        // (`replication_seed/install/staging.rs` uses `fjall_raw`).
        let backing = BoxRepoFactory::fjall_raw(&image).create().await.unwrap();
        let instance = RepoInstance::new("main".into(), backing, Vec::new());
        let writer = SeedImageWriter::begin(
            instance.clone(),
            receipt(),
            vec![SeedTable {
                name: "items".into(),
                live_records: 0,
            }],
        )
        .await
        .unwrap();
        let store = home
            .prepare_seed_schema(
                &instance,
                "app",
                "main",
                &[("items".into(), QueryValue::Null, 0)],
            )
            .await
            .unwrap();
        writer.finish(std::slice::from_ref(&store)).await.unwrap();
        home.register_seed_image(&instance, &receipt(), &image)
            .await
            .unwrap();
        let record = home
            .system_store
            .load_repository_record("app", "main")
            .await
            .unwrap()
            .expect("seeded repo registered");
        assert_eq!(
            record["engine"].as_str(),
            Some("fjall_raw"),
            "catalogue must record the RAW fjall kind"
        );
        instance.stop_journal_writer_for_test().await.unwrap();
    });
    drop(rt);
    // Reopen through the NORMAL boot path (`factory_from_meta` over the
    // saved catalogue record, via `ShamirDb::init`).
    let rt = runtime();
    rt.block_on(async {
        let reopened = ShamirDb::init(SystemStoreConfig::Fjall(catalogue))
            .await
            .unwrap();
        let repo = reopened
            .get_db("app")
            .and_then(|db| db.get_repo("main"))
            .expect("seeded repo reopens through the boot path");
        // The reopened backend must be atomic-batch capable — this is what
        // `admit_follower` gates on.
        assert!(repo.seed_backing_repo().supports_atomic_batch());
        // v3 seed baseline intact: durable bookmark = seed source version,
        // no outstanding redo receipt.
        assert_eq!(repo.replication_bookmark().await.unwrap(), 73);
        let info = repo.tx_info_store().await.unwrap();
        let baseline = load_bookmark_v3(&info)
            .await
            .unwrap()
            .expect("v3 bookmark written by the seed image");
        assert_eq!(baseline.bookmark, 73);
        assert!(baseline.outstanding_receipt.is_none());
        // One replicated first batch applies (`Applied`), and the durable
        // bookmark advances to the batch's upstream end.
        repo.get_table("items").await.unwrap();
        let event = put_event("hello", RecordId([2u8; 16]), 74);
        let outcome = apply_replicated_batch(
            &repo,
            &[ReplicatedEvent {
                event,
                field_bindings: Vec::new(),
            }],
            74,
            ApplyPolicy::Trusted,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            ApplyOutcome::Applied { local_version: 1 },
            "first replicated batch must apply on the seeded follower"
        );
        assert_eq!(
            repo.replication_bookmark().await.unwrap(),
            74,
            "bookmark must advance to the batch's upstream end"
        );
    });
}
