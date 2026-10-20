#![cfg(unix)]

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig, SeedInstallState};
use shamir_db::query::filter::FilterContext;
use shamir_db::query::read::ReadQuery;
use shamir_db::{ShamirDb, SystemStoreConfig};
use tokio::runtime::Builder;

use crate::replication_seed::exporter::export_open;
use crate::replication_seed::{install_subscription, SeedSubscriptionArgs};

#[test]
fn install_cli_final_step_is_paused_durable_and_exact_retry_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let artifact = dir.path().join("artifact");
    let data = dir.path().join("data");
    let args = SeedSubscriptionArgs {
        subscription: "sub".into(),
        upstream: "peer".into(),
        publication: "pub".into(),
        profile: "prof".into(),
    };
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let digest = runtime.block_on(async {
        let source = ShamirDb::init_memory().await.unwrap();
        source.create_db("app").await;
        source
            .add_repo("app", RepoConfig::new("main", BoxRepoFactory::in_memory()))
            .await
            .unwrap();
        source
            .add_table("app", "main", "empty", false)
            .await
            .unwrap();
        let repo = source.get_db("app").unwrap().get_repo("main").unwrap();
        let table = repo.get_table("empty").await.unwrap();
        let value: shamir_db::types::value::QueryValue =
            shamir_query_builder::doc! { "value" => 1 }.into();
        let inner = shamir_types::codecs::interned::query_value_to_inner(
            &value,
            table.interner_for_test().get().await.unwrap(),
        )
        .unwrap();
        table
            .mvcc_store_for_test()
            .unwrap()
            .set_versioned_no_wait(
                shamir_db::storage::types::RecordKey::from_slice(b"seed-row"),
                inner.to_bytes().unwrap(),
            )
            .await
            .unwrap();
        export_open(&source, "app", "main", &artifact, 1024)
            .await
            .unwrap()
    });
    // Simulate the durable boundary after repository registration but before
    // subscription persistence. Exact retry must finish the missing last step.
    runtime
        .block_on(crate::replication_seed::install(
            &data, "app", "main", &artifact, &digest,
        ))
        .unwrap();
    // `install` leaves its store handles to background tasks that only make
    // progress while the runtime is polled, so the Fjall lock is released a
    // few polls later: retry the open instead of racing the teardown.
    runtime.block_on(async {
        for _ in 0..100 {
            if install_subscription(&data, "app", "main", &artifact, &digest, &args)
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        install_subscription(&data, "app", "main", &artifact, &digest, &args)
            .await
            .expect("seed store still locked after retries");
    });
    drop(runtime);
    for _ in 0..2 {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            install_subscription(&data, "app", "main", &artifact, &digest, &args)
                .await
                .unwrap();
            let home =
                ShamirDb::init_seed(SystemStoreConfig::Fjall(data.join("shamir_db_meta.redb")))
                    .await
                    .unwrap();
            let table = home.system_store().subscriptions_table().await.unwrap();
            let refs = shamir_collections::TMap::default();
            let ctx = FilterContext::new(table.interner_for_test().get().await.unwrap(), &refs);
            let result = table
                .read(&ReadQuery::new("subscriptions"), &ctx)
                .await
                .unwrap();
            assert_eq!(result.records.len(), 1);
            let row = result.records[0].as_value();
            assert_eq!(row["state"].as_str(), Some("paused"));
            assert_eq!(
                row["seed_receipt"]["artifact_digest"].as_str(),
                Some(digest.as_str())
            );
            let repo = home.get_db("app").unwrap().get_repo("main").unwrap();
            assert!(matches!(
                SeedInstallState::load(&repo.tx_info_store().await.unwrap())
                    .await
                    .unwrap(),
                Some(SeedInstallState::Complete(_))
            ));
            assert_eq!(repo.consistent_version().await.unwrap(), 1);
        });
    }
}
