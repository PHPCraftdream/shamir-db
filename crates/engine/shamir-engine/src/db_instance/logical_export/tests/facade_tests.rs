use super::super::{LogicalExport, LogicalRows, LogicalTable, MaintenanceGate};
use crate::db_instance::db_instance::DbInstance;
use crate::repo::{BoxRepoFactory, RepoConfig};
use crate::table::{TableConfig, TableManager};
use bytes::Bytes;
use futures::FutureExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_types::codecs::interned::query_value_to_inner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};
use std::sync::Arc;

async fn table(name: &str) -> TableManager {
    TableManager::create(
        name.into(),
        Arc::new(InMemoryStore::new()),
        Arc::new(InMemoryStore::new()),
    )
    .await
    .unwrap()
}

async fn insert(
    table: &TableManager,
    value: &QueryValue,
) -> shamir_types::types::record_id::RecordId {
    let interner = table.interner_for_test().get().await.unwrap();
    let inner = query_value_to_inner(value, interner).unwrap();
    table.insert(&inner).await.unwrap()
}

#[tokio::test]
async fn fixture_pages_preserve_names_keys_order_binary_and_live_rows() {
    let table = table("records").await;
    let mut fields = new_map();
    fields.insert("z-last-lexically".into(), QueryValue::Int(42));
    fields.insert("a-first-lexically".into(), QueryValue::Bin(vec![0, 255]));
    fields.insert(
        "nested".into(),
        QueryValue::List(vec![QueryValue::Bool(true), QueryValue::Null]),
    );
    let expected = QueryValue::Map(fields);
    let id = insert(&table, &expected).await;
    let deleted = insert(&table, &QueryValue::Str("deleted".into())).await;
    table.delete(deleted).await.unwrap();
    MaintenanceGate::default()
        .with_freeze(|guard| {
            async move {
                let mut rows = LogicalRows::new(table, 0, guard, false).await?;
                let page = rows.next_page().await?.unwrap();
                assert_eq!(page.len(), 1);
                assert_eq!(page[0].0.as_ref(), &id.0);
                assert_eq!(page[0].1, expected);
                let QueryValue::Map(ref fields) = page[0].1 else {
                    panic!("expected map");
                };
                assert_eq!(fields.keys().next().unwrap(), "z-last-lexically");
                assert!(rows.next_page().await?.is_none());
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn settings_only_exports_namespace_metadata() {
    let table = table("settings").await;
    for key in ["root_meta", "fn_namespace_meta", "next_group_id", "secret"] {
        let mut fields = new_map();
        fields.insert("key".into(), QueryValue::Str(key.into()));
        fields.insert("value".into(), QueryValue::Int(7));
        insert(&table, &QueryValue::Map(fields)).await;
    }
    MaintenanceGate::default()
        .with_freeze(|guard| {
            async move {
                let mut rows = LogicalRows::new(table, 0, guard, true).await?;
                let mut names = Vec::new();
                while let Some(page) = rows.next_page().await? {
                    names.push(page[0].1.get("key").unwrap().as_str().unwrap().to_string());
                }
                names.sort();
                assert_eq!(names, ["fn_namespace_meta", "root_meta"]);
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn corrupt_record_fails_closed_and_poisoned_cursor_stops() {
    let table = table("records").await;
    table
        .data_store_for_test()
        .set(
            Bytes::from_static(b"raw-key").into(),
            Bytes::from_static(&[0xc1]),
        )
        .await
        .unwrap();
    MaintenanceGate::default()
        .with_freeze(|guard| {
            async move {
                let mut rows = LogicalRows::new(table, 0, guard, false).await?;
                assert!(rows.next_page().await.is_err());
                assert!(rows.next_page().await?.is_none());
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn unresolved_interner_key_is_not_silently_dropped() {
    let table = table("records").await;
    let mut fields = new_map();
    fields.insert(
        shamir_types::core::interner::InternerKey::new(u64::MAX),
        InnerValue::Int(1),
    );
    table.insert(&InnerValue::Map(fields)).await.unwrap();
    MaintenanceGate::default()
        .with_freeze(|guard| {
            async move {
                let mut rows = LogicalRows::new(table, 0, guard, false).await?;
                assert!(rows.next_page().await.is_err());
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn metadata_keeps_ttl_and_resolves_all_base_index_names() {
    let table = table("records").await;
    let config = shamir_storage::storage_membuffer::MemBufferConfig {
        ttl_ms: Some(123_456),
        ..Default::default()
    };
    table.set_buffer_config(&config).await.unwrap();
    table.create_index("regular", &["field"]).await.unwrap();
    table
        .create_unique_index("unique", &["field"])
        .await
        .unwrap();
    table
        .create_sorted_index("sorted", &["field"])
        .await
        .unwrap();
    MaintenanceGate::default()
        .with_freeze(|guard| {
            async move {
                let metadata = LogicalTable::new(table, guard);
                assert_eq!(
                    metadata.buffer_config().await?.unwrap().ttl_ms,
                    Some(123_456)
                );
                let regular = metadata.regular_indexes().next().unwrap();
                assert_eq!(metadata.name(regular.name_interned).await?, "regular");
                assert_eq!(metadata.name(regular.paths[0].path[0]).await?, "field");
                let unique = metadata.unique_indexes().next().unwrap();
                assert_eq!(metadata.name(unique.name_interned).await?, "unique");
                let sorted = metadata.sorted_indexes();
                assert_eq!(metadata.name(sorted[0].name_interned).await?, "sorted");
                assert!(metadata.next_index2(0).await?.is_none());
                assert!(metadata.bindings().is_empty());
                assert!(metadata.transforms().is_empty());
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn source_length_bomb_is_rejected_before_tree_decode() {
    let table = table("records").await;
    table
        .data_store_for_test()
        .set(
            Bytes::from_static(b"key").into(),
            Bytes::from_static(&[0xdd, 255, 255, 255, 255]),
        )
        .await
        .unwrap();
    MaintenanceGate::default()
        .with_freeze(|guard| {
            async move {
                let mut rows = LogicalRows::new(table, 0, guard, false).await?;
                assert!(rows.next_page().await.is_err());
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn capture_inventories_and_rejects_excluded_catalogues() {
    let db =
        DbInstance::with_repos(vec![RepoConfig::new("repo", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("records"))])
        .await
        .unwrap();
    let system =
        DbInstance::with_repos(vec![RepoConfig::new("system", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("tables"))])
        .await
        .unwrap()
        .get_repo("system")
        .unwrap();
    db.get_table("repo", "records").await.unwrap();
    system.get_table("tables").await.unwrap();
    MaintenanceGate::default()
        .with_freeze(|guard| {
            async move {
                let export = LogicalExport::capture(&db, &system, guard).await?;
                assert_eq!(export.repository_versions().count(), 1);
                assert_eq!(export.table_names("repo")?, ["records"]);
                assert!(export.catalogue_rows("users").await.is_err());
                assert!(export.catalogue_rows("roles").await.is_err());
                assert!(export.catalogue_rows("subscriptions").await.is_err());
                assert!(export
                    .catalogue_rows("tables")
                    .await?
                    .next_page()
                    .await?
                    .is_none());
                assert!(export
                    .records("repo", "records")
                    .await?
                    .next_page()
                    .await?
                    .is_none());
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
}
