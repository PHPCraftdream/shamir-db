use std::sync::Arc;

use shamir_engine::meta::repo_identity::RepoIdentity;
use shamir_engine::repo::{BoxRepo, RepoInstance, SeedImageWriter, SeedReceipt, SeedTable};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::RecordKey;
use shamir_types::types::value::QueryValue;

use crate::shamir_db::shamir_db::schema_management::parse_schema;
use crate::ShamirDb;

use super::super::records_codec::decode_schema_records;

#[tokio::test]
async fn names_schema_is_compiled_and_durable_before_complete() {
    let home = ShamirDb::init_memory().await.unwrap();
    let image = RepoInstance::new(
        "main".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    );
    let receipt = SeedReceipt {
        artifact_digest: "a".repeat(64),
        source_db: "app".into(),
        source_repo: "main".into(),
        source_identity: RepoIdentity {
            lineage_id: [8; 16],
            history_epoch: 1,
        },
        schema_digest: "b".repeat(64),
        source_version: 73,
        local_baseline: 1,
    };
    let mut writer = SeedImageWriter::begin(
        image.clone(),
        receipt,
        vec![SeedTable {
            name: "items".into(),
            live_records: 0,
        }],
    )
    .await
    .unwrap();
    writer.binding(700, "name").await.unwrap();
    let rule: QueryValue = shamir_query_builder::doc! {
        "path" => QueryValue::List(vec![QueryValue::Str("name".into())]),
        "type" => "string",
        "required" => true,
    }
    .into();
    let schema = QueryValue::List(vec![rule]);
    let store = home
        .prepare_seed_schema(&image, "app", "main", &[("items".into(), schema, 9)])
        .await
        .unwrap();
    assert!(home.system_store.load_tables().await.unwrap().is_empty());
    let bytes = store.get(RecordKey::from_slice(b"schemas")).await.unwrap();
    let records = decode_schema_records(&bytes).unwrap();
    assert_eq!(records[0]["schema_version"].as_i64(), Some(9));
    let manager = image.repo_interner().await.unwrap();
    let interner = manager.get().await.unwrap();
    let local = interner.get_ind("name").unwrap().id();
    assert_ne!(local, 700);
    assert_eq!(
        records[0]["schema"][0]["path"][0].as_i64(),
        Some(local as i64)
    );
    let rules = parse_schema(&records[0]["schema"], interner).unwrap();
    assert_eq!(rules[0].path, vec!["name"]);
    assert_eq!(image.replication_bookmark().await.unwrap(), 0);
    writer.finish(&[store]).await.unwrap();
    assert_eq!(image.replication_bookmark().await.unwrap(), 73);
}
