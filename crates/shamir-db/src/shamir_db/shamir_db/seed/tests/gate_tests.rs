#![cfg(all(unix, feature = "fjall"))]

use shamir_engine::meta::repo_identity::RepoIdentity;
use shamir_engine::repo::{
    BoxRepoFactory, RepoFactory, RepoInstance, SeedImageWriter, SeedInstallState, SeedReceipt,
};
use shamir_query_builder::doc;
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
                .save_repository("app", "main", "fjall", image.to_str(), &metadata)
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
