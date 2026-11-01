use crate::access::Actor;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::{ShamirDb, SystemStoreConfig};
use crate::types::value::{InnerValue, QueryValue};
use futures::StreamExt;
use shamir_types::types::record_id::RecordId;

async fn write_rows(table: &crate::engine::table::TableManager, values: &[&str]) -> Vec<RecordId> {
    let records = values
        .iter()
        .map(|value| InnerValue::Str((*value).into()))
        .collect::<Vec<_>>();
    table.insert_many(&records).await.unwrap()
}

async fn listed_rows(table: &crate::engine::table::TableManager) -> Vec<(RecordId, InnerValue)> {
    table
        .list_stream(32)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flat_map(Result::unwrap)
        .map(|(id, value)| (id, value.into_inner().unwrap()))
        .collect()
}

async fn assert_rows(
    table: &crate::engine::table::TableManager,
    expected: &[(RecordId, InnerValue)],
) {
    let rows = listed_rows(table).await;
    assert_eq!(rows.len(), expected.len());
    for (id, value) in expected {
        assert!(rows.iter().any(|row| row == &(*id, value.clone())));
    }
}

#[tokio::test]
async fn drop_recreate_separates_store_incarnations() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("identity").await.unwrap();
    db.add_repo(
        "identity",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    db.add_table("identity", "main", "items", false)
        .await
        .unwrap();
    let old = db
        .system_store()
        .load_table_record("identity", "main", "items")
        .await
        .unwrap()
        .unwrap();
    let old_id = old
        .get("store_id")
        .and_then(QueryValue::as_str)
        .unwrap()
        .to_owned();
    let dbi = db.get_db("identity").unwrap();
    let old_table = dbi.get_table("main", "items").await.unwrap();
    let old_ids = write_rows(&old_table, &["stale-1", "stale-2", "stale-3"]).await;
    db.drop_table("identity", "main", "items").await.unwrap();
    db.add_table("identity", "main", "items", false)
        .await
        .unwrap();
    let new = db
        .system_store()
        .load_table_record("identity", "main", "items")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        old_id,
        new.get("store_id").and_then(QueryValue::as_str).unwrap()
    );
    let recreated_table = dbi.get_table("main", "items").await.unwrap();
    let visible_rows = recreated_table
        .list_stream(32)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flat_map(Result::unwrap)
        .collect::<Vec<_>>();
    assert!(visible_rows.is_empty());
    assert_eq!(old_ids.len(), 3);
}

#[tokio::test]
async fn rename_recreate_and_swap_names_preserves_each_incarnation() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("rename").await.unwrap();
    db.add_repo(
        "rename",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    db.add_table("rename", "main", "a", false).await.unwrap();
    let dbi = db.get_db("rename").unwrap();
    let old_ids = write_rows(
        &dbi.get_table("main", "a").await.unwrap(),
        &["original-a", "original-b"],
    )
    .await;
    db.rename_table_as("rename", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    assert_rows(
        &dbi.get_table("main", "b").await.unwrap(),
        &[
            (old_ids[0], InnerValue::Str("original-a".into())),
            (old_ids[1], InnerValue::Str("original-b".into())),
        ],
    )
    .await;
    assert!(dbi.get_table("main", "a").await.is_err());
    db.add_table("rename", "main", "a", false).await.unwrap();
    let recreated_ids =
        write_rows(&dbi.get_table("main", "a").await.unwrap(), &["recreated-a"]).await;
    db.rename_table_as("rename", "main", "b", "c", Actor::System)
        .await
        .unwrap();
    db.rename_table_as("rename", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    db.rename_table_as("rename", "main", "c", "a", Actor::System)
        .await
        .unwrap();
    let old_a = dbi.get_table("main", "a").await.unwrap();
    let new_a = dbi.get_table("main", "b").await.unwrap();
    let old_values = old_a
        .list_stream(32)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flat_map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(old_values.len(), 2);
    let values = old_values
        .iter()
        .map(|(_, value)| value.into_inner().unwrap())
        .collect::<Vec<_>>();
    assert!(values.iter().all(|value| [
        InnerValue::Str("original-a".into()),
        InnerValue::Str("original-b".into())
    ]
    .contains(value)));
    let ids = old_values
        .iter()
        .map(|(id, _)| *id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 2);
    assert!(old_ids.iter().all(|id| ids.contains(id)));
    let new_values = new_a
        .list_stream(32)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flat_map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(new_values.len(), 1);
    assert_eq!(new_values[0].0, recreated_ids[0]);
    assert_eq!(
        new_values[0].1.into_inner().unwrap(),
        InnerValue::Str("recreated-a".into())
    );
}

#[tokio::test]
async fn durability_tail_failure_retry_reconciles_registration_and_is_idempotent() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("retry").await.unwrap();
    db.add_repo(
        "retry",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    db.add_table("retry", "main", "a", false).await.unwrap();
    let source = db
        .get_db("retry")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .get_table("a")
        .await
        .unwrap();
    let source_ids = write_rows(&source, &["retry-a", "retry-b"]).await;
    let expected = vec![
        (source_ids[0], InnerValue::Str("retry-a".into())),
        (source_ids[1], InnerValue::Str("retry-b".into())),
    ];
    let repo = db.get_db("retry").unwrap().get_repo("main").unwrap();
    let catalogue = db.system_store();
    let source_record = catalogue
        .load_table_record("retry", "main", "a")
        .await
        .unwrap()
        .unwrap();
    let source_id = source_record
        .get("store_id")
        .unwrap()
        .as_str()
        .unwrap()
        .parse::<RecordId>()
        .unwrap();
    let persist_error = repo
        .rename_table_stores_with("a", "b", move |destination| async move {
            catalogue
                .rename_table_row(
                    "retry",
                    "main",
                    "a",
                    "b",
                    Some(&source_id),
                    destination.store_id.as_ref().unwrap(),
                    &source_record,
                )
                .await?;
            Err(crate::DbError::Storage(
                "injected durability-tail failure".into(),
            ))
        })
        .await;
    assert!(persist_error.is_err());
    assert!(repo.has_table("a"));
    assert!(!repo.has_table("b"));
    assert_rows(&source, &expected).await;
    assert!(db
        .system_store()
        .load_table_record("retry", "main", "b")
        .await
        .unwrap()
        .is_some());
    assert!(db
        .system_store()
        .load_table_record("retry", "main", "a")
        .await
        .unwrap()
        .is_none());
    db.rename_table_as("retry", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    let renamed = db.get_table("retry", "main", "b").await.unwrap();
    assert_rows(&renamed, &expected).await;
    assert!(db
        .get_db("retry")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .get_table("a")
        .await
        .is_err());
    db.rename_table_as("retry", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    assert_rows(&renamed, &expected).await;
}

#[tokio::test]
async fn precommit_persist_failure_retry_uses_fresh_destination_id() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("precommit").await.unwrap();
    db.add_repo(
        "precommit",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    db.add_table("precommit", "main", "a", false).await.unwrap();
    let repo = db.get_db("precommit").unwrap().get_repo("main").unwrap();
    let source = repo.get_table("a").await.unwrap();
    let ids = write_rows(&source, &["source-1", "source-2"]).await;
    let expected = vec![
        (ids[0], InnerValue::Str("source-1".into())),
        (ids[1], InnerValue::Str("source-2".into())),
    ];
    let failed_id = std::sync::Arc::new(std::sync::Mutex::new(None));
    let failed_id_capture = failed_id.clone();
    assert!(repo
        .rename_table_stores_with("a", "b", move |destination| async move {
            *failed_id_capture.lock().unwrap() = destination.store_id;
            Err(crate::DbError::Storage("injected precommit failure".into()))
        })
        .await
        .is_err());
    let failed_id = failed_id.lock().unwrap().unwrap();
    assert!(repo.has_table("a"));
    assert!(!repo.has_table("b"));
    assert_rows(&source, &expected).await;
    assert!(db
        .system_store()
        .load_table_record("precommit", "main", "a")
        .await
        .unwrap()
        .is_some());
    assert!(db
        .system_store()
        .load_table_record("precommit", "main", "b")
        .await
        .unwrap()
        .is_none());

    db.rename_table_as("precommit", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    let destination_record = db
        .system_store()
        .load_table_record("precommit", "main", "b")
        .await
        .unwrap()
        .unwrap();
    let retry_id = destination_record
        .get("store_id")
        .unwrap()
        .as_str()
        .unwrap()
        .parse::<RecordId>()
        .unwrap();
    assert_ne!(retry_id, failed_id);
    assert_rows(
        &db.get_table("precommit", "main", "b").await.unwrap(),
        &expected,
    )
    .await;
}

#[tokio::test]
async fn retry_rejects_recreated_source_and_preserves_both_incarnations() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("recreate").await.unwrap();
    db.add_repo(
        "recreate",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    db.add_table("recreate", "main", "a", false).await.unwrap();
    let original = db.get_table("recreate", "main", "a").await.unwrap();
    let original_ids = write_rows(&original, &["original-1", "original-2"]).await;
    let original_expected = vec![
        (original_ids[0], InnerValue::Str("original-1".into())),
        (original_ids[1], InnerValue::Str("original-2".into())),
    ];
    db.rename_table_as("recreate", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    db.add_table("recreate", "main", "a", false).await.unwrap();
    let recreated = db.get_table("recreate", "main", "a").await.unwrap();
    let recreated_ids = write_rows(&recreated, &["recreated"]).await;
    let recreated_expected = vec![(recreated_ids[0], InnerValue::Str("recreated".into()))];
    assert!(db
        .rename_table_as("recreate", "main", "a", "b", Actor::System)
        .await
        .is_err());
    assert_rows(
        &db.get_table("recreate", "main", "a").await.unwrap(),
        &recreated_expected,
    )
    .await;
    assert_rows(
        &db.get_table("recreate", "main", "b").await.unwrap(),
        &original_expected,
    )
    .await;
}

#[tokio::test]
async fn retry_rejects_unrelated_destination_without_changing_rows() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("conflict").await.unwrap();
    db.add_repo(
        "conflict",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    db.add_table("conflict", "main", "a", false).await.unwrap();
    db.add_table("conflict", "main", "b", false).await.unwrap();
    let a = db.get_table("conflict", "main", "a").await.unwrap();
    let b = db.get_table("conflict", "main", "b").await.unwrap();
    let a_ids = write_rows(&a, &["source"]).await;
    let b_ids = write_rows(&b, &["destination"]).await;
    let a_expected = vec![(a_ids[0], InnerValue::Str("source".into()))];
    let b_expected = vec![(b_ids[0], InnerValue::Str("destination".into()))];
    assert!(db
        .rename_table_as("conflict", "main", "a", "b", Actor::System)
        .await
        .is_err());
    assert_rows(&a, &a_expected).await;
    assert_rows(&b, &b_expected).await;
}

#[tokio::test]
async fn retry_removes_stale_source_catalogue_row_only() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("stale").await.unwrap();
    db.add_repo(
        "stale",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    db.add_table("stale", "main", "a", false).await.unwrap();
    let source = db.get_table("stale", "main", "a").await.unwrap();
    let ids = write_rows(&source, &["dest-1", "dest-2"]).await;
    let expected = vec![
        (ids[0], InnerValue::Str("dest-1".into())),
        (ids[1], InnerValue::Str("dest-2".into())),
    ];
    let catalogue = db.system_store();
    let source_row = catalogue
        .load_table_record("stale", "main", "a")
        .await
        .unwrap()
        .unwrap();
    db.rename_table_as("stale", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    catalogue.save_table_meta(&source_row).await.unwrap();
    db.rename_table_as("stale", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    assert!(catalogue
        .load_table_record("stale", "main", "a")
        .await
        .unwrap()
        .is_none());
    assert!(catalogue
        .load_table_record("stale", "main", "b")
        .await
        .unwrap()
        .is_some());
    assert_rows(
        &db.get_table("stale", "main", "b").await.unwrap(),
        &expected,
    )
    .await;
}

#[tokio::test]
async fn legacy_idless_stale_source_row_is_removed_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let db = ShamirDb::init(SystemStoreConfig::Fjall(dir.path().join("system")))
        .await
        .unwrap();
    db.create_db("legacy-retry").await.unwrap();
    db.add_repo(
        "legacy-retry",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("a")),
    )
    .await
    .unwrap();
    let source = db.get_table("legacy-retry", "main", "a").await.unwrap();
    let ids = write_rows(&source, &["legacy-a", "legacy-b"]).await;
    let expected = vec![
        (ids[0], InnerValue::Str("legacy-a".into())),
        (ids[1], InnerValue::Str("legacy-b".into())),
    ];
    let catalogue = db.system_store();
    let source_row = catalogue
        .load_table_record("legacy-retry", "main", "a")
        .await
        .unwrap()
        .unwrap();
    assert!(source_row.get("store_id").is_none());
    db.rename_table_as("legacy-retry", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    catalogue.save_table_meta(&source_row).await.unwrap();
    db.rename_table_as("legacy-retry", "main", "a", "b", Actor::System)
        .await
        .unwrap();
    assert!(catalogue
        .load_table_record("legacy-retry", "main", "a")
        .await
        .unwrap()
        .is_none());
    assert_rows(
        &db.get_table("legacy-retry", "main", "b").await.unwrap(),
        &expected,
    )
    .await;
}

#[tokio::test]
async fn legacy_row_stays_idless_and_malformed_id_is_rejected() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("legacy").await.unwrap();
    db.add_repo(
        "legacy",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("old")),
    )
    .await
    .unwrap();
    let row = db
        .system_store()
        .load_table_record("legacy", "main", "old")
        .await
        .unwrap()
        .unwrap();
    assert!(row.get("store_id").is_none());
    let mut malformed = row.clone();
    if let QueryValue::Map(fields) = &mut malformed {
        fields.insert("store_id".into(), QueryValue::Bool(true));
    }
    assert!(matches!(
        db.system_store().save_table_meta(&malformed).await,
        Err(crate::DbError::Internal(_))
    ));
    assert_eq!(
        db.system_store()
            .load_table_record("legacy", "main", "old")
            .await
            .unwrap()
            .unwrap(),
        row
    );
}
