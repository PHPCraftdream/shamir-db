use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::{ShamirDb, SystemStoreConfig};
use crate::types::value::{InnerValue, QueryValue};
use futures::StreamExt;
use shamir_query_builder::doc;
use shamir_storage::types::Repo;
use shamir_types::types::record_id::RecordId;
use tokio::runtime::Builder;

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

fn assert_rows(actual: &[(RecordId, InnerValue)], expected: &[(RecordId, InnerValue)]) {
    assert_eq!(actual.len(), expected.len());
    for row in expected {
        assert!(actual.contains(row));
    }
}

#[test]
fn rename_drop_recreate_uses_fresh_store_incarnation() {
    let dir = tempfile::tempdir().unwrap();
    let system_path = dir.path().join("system");
    let repo_path = dir.path().join("repo");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let (old_ids, new_ids, old_store_id, new_store_id) = runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
            .await
            .unwrap();
        db.create_db("p34").await.unwrap();
        db.add_repo(
            "p34",
            RepoConfig::new("main", BoxRepoFactory::fjall(&repo_path)),
        )
        .await
        .unwrap();
        db.add_table("p34", "main", "items", false).await.unwrap();
        let instance = db.get_db("p34").unwrap();
        let table = instance.get_table("main", "items").await.unwrap();
        let old_ids = table
            .insert_many(&[
                InnerValue::Str("old-1".into()),
                InnerValue::Str("old-2".into()),
            ])
            .await
            .unwrap();
        let expected_old = vec![
            (old_ids[0], InnerValue::Str("old-1".into())),
            (old_ids[1], InnerValue::Str("old-2".into())),
        ];
        db.rename_table("p34", "main", "items", "saved")
            .await
            .unwrap();
        let saved = instance.get_table("main", "saved").await.unwrap();
        assert_rows(&listed_rows(&saved).await, &expected_old);
        assert_eq!(
            saved
                .get_many(&old_ids)
                .await
                .unwrap()
                .into_iter()
                .map(|value| value.unwrap())
                .collect::<Vec<_>>(),
            vec![
                InnerValue::Str("old-1".into()),
                InnerValue::Str("old-2".into())
            ]
        );
        db.add_table("p34", "main", "items", false).await.unwrap();
        let recreated = instance.get_table("main", "items").await.unwrap();
        let doomed_ids = recreated
            .insert_many(&[
                InnerValue::Str("doomed-1".into()),
                InnerValue::Str("doomed-2".into()),
            ])
            .await
            .unwrap();
        db.drop_table("p34", "main", "items").await.unwrap();
        db.add_table("p34", "main", "items", false).await.unwrap();
        let fresh = instance.get_table("main", "items").await.unwrap();
        let new_ids = fresh
            .insert_many(&[InnerValue::Str("new-only".into())])
            .await
            .unwrap();
        assert_eq!(new_ids.len(), 1);
        assert!(fresh
            .get_many(&doomed_ids)
            .await
            .unwrap()
            .into_iter()
            .all(|value| value.is_none()));
        db.flush_all_for_test().await.unwrap();
        let old_record = db
            .system_store()
            .load_table_record("p34", "main", "saved")
            .await
            .unwrap()
            .unwrap();
        let new_record = db
            .system_store()
            .load_table_record("p34", "main", "items")
            .await
            .unwrap()
            .unwrap();
        (
            old_ids,
            new_ids,
            old_record
                .get("store_id")
                .and_then(QueryValue::as_str)
                .unwrap()
                .to_owned(),
            new_record
                .get("store_id")
                .and_then(QueryValue::as_str)
                .unwrap()
                .to_owned(),
        )
    });
    // Runtime teardown drops background owners as well as all foreground
    // handles, so the next open is a genuine cold reopen rather than an
    // in-process handle-retention check.
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path))
            .await
            .unwrap();
        let instance = db.get_db("p34").unwrap();
        let saved = instance.get_table("main", "saved").await.unwrap();
        let expected_old = vec![
            (old_ids[0], InnerValue::Str("old-1".into())),
            (old_ids[1], InnerValue::Str("old-2".into())),
        ];
        assert_rows(&listed_rows(&saved).await, &expected_old);
        assert_eq!(
            saved
                .get_many(&old_ids)
                .await
                .unwrap()
                .into_iter()
                .map(|value| value.unwrap())
                .collect::<Vec<_>>(),
            vec![
                InnerValue::Str("old-1".into()),
                InnerValue::Str("old-2".into())
            ]
        );
        let table = instance.get_table("main", "items").await.unwrap();
        let expected_new = vec![(new_ids[0], InnerValue::Str("new-only".into()))];
        assert_rows(&listed_rows(&table).await, &expected_new);
        assert_eq!(
            table
                .get_many(&new_ids)
                .await
                .unwrap()
                .into_iter()
                .map(|value| value.unwrap())
                .collect::<Vec<_>>(),
            vec![InnerValue::Str("new-only".into())]
        );
        let saved_record = db
            .system_store()
            .load_table_record("p34", "main", "saved")
            .await
            .unwrap()
            .unwrap();
        let item_record = db
            .system_store()
            .load_table_record("p34", "main", "items")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            saved_record.get("store_id").and_then(QueryValue::as_str),
            Some(old_store_id.as_str())
        );
        assert_eq!(
            item_record.get("store_id").and_then(QueryValue::as_str),
            Some(new_store_id.as_str())
        );
    });
    drop(runtime);
}

#[test]
fn legacy_catalogue_row_without_store_id_remains_idless() {
    let dir = tempfile::tempdir().unwrap();
    let system_path = dir.path().join("system");
    let repo_path = dir.path().join("repo");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let ids = runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
            .await
            .unwrap();
        db.create_db("legacy").await.unwrap();
        db.add_repo(
            "legacy",
            RepoConfig::new("main", BoxRepoFactory::fjall_raw(&repo_path)),
        )
        .await
        .unwrap();
        let facade_repo = db.get_db("legacy").unwrap().get_repo("main").unwrap();
        let backing = facade_repo.seed_backing_repo();
        for name in ["__data__old", "__info__old", "__history__old"] {
            backing.store_get(name).await.unwrap();
        }
        let permit = facade_repo.write_domain().admit().await.unwrap();
        facade_repo
            .add_table_admitted(TableConfig::new("old"), &permit)
            .await
            .unwrap();
        drop(permit);
        assert_eq!(facade_repo.table_config("old").unwrap().store_id, None);

        let table = facade_repo.get_table("old").await.unwrap();
        let ids = table
            .insert_many(&[
                InnerValue::Str("legacy-1".into()),
                InnerValue::Str("legacy-2".into()),
            ])
            .await
            .unwrap();
        assert_eq!(ids.len(), 2);
        assert!(facade_repo.current_commit_version().await.unwrap() > 0);
        let stores = backing.stores_list().await.unwrap();
        for name in ["__data__old", "__info__old", "__history__old"] {
            assert!(stores.iter().any(|stored| stored == name));
        }
        assert!(!stores.iter().any(|stored| {
            stored.starts_with("__data__old--")
                || stored.starts_with("__info__old--")
                || stored.starts_with("__history__old--")
        }));
        let record: QueryValue = doc! {
            "db_name" => "legacy",
            "repo_name" => "main",
            "table_name" => "old",
            "enable_indexes" => false,
        }
        .into();
        db.system_store().save_table_meta(&record).await.unwrap();
        db.flush_all_for_test().await.unwrap();
        drop(backing);
        ids
    });
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let renamed_store_id = runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
            .await
            .unwrap();
        let instance = db.get_db("legacy").unwrap();
        let backing = instance.get_repo("main").unwrap().seed_backing_repo();
        let stores = backing.stores_list().await.unwrap();
        for name in ["__data__old", "__info__old", "__history__old"] {
            assert!(stores.iter().any(|stored| stored == name));
        }
        assert!(!stores.iter().any(|stored| {
            stored.starts_with("__data__old--")
                || stored.starts_with("__info__old--")
                || stored.starts_with("__history__old--")
        }));
        let old = instance.get_table("main", "old").await.unwrap();
        let expected = vec![
            (ids[0], InnerValue::Str("legacy-1".into())),
            (ids[1], InnerValue::Str("legacy-2".into())),
        ];
        assert_rows(&listed_rows(&old).await, &expected);
        assert!(stores.iter().any(|stored| stored == "__data__old"));
        assert_eq!(
            old.get_many(&ids)
                .await
                .unwrap()
                .into_iter()
                .map(|value| value.unwrap())
                .collect::<Vec<_>>(),
            vec![
                InnerValue::Str("legacy-1".into()),
                InnerValue::Str("legacy-2".into())
            ]
        );
        let row = db
            .system_store()
            .load_table_record("legacy", "main", "old")
            .await
            .unwrap()
            .unwrap();
        assert!(row.get("store_id").is_none());
        db.rename_table("legacy", "main", "old", "renamed")
            .await
            .unwrap();
        db.flush_all_for_test().await.unwrap();
        let renamed = instance.get_table("main", "renamed").await.unwrap();
        assert_rows(&listed_rows(&renamed).await, &expected);
        assert_eq!(
            renamed
                .get_many(&ids)
                .await
                .unwrap()
                .into_iter()
                .map(|value| value.unwrap())
                .collect::<Vec<_>>(),
            vec![
                InnerValue::Str("legacy-1".into()),
                InnerValue::Str("legacy-2".into())
            ]
        );
        let row = db
            .system_store()
            .load_table_record("legacy", "main", "renamed")
            .await
            .unwrap()
            .unwrap();
        row.get("store_id")
            .and_then(QueryValue::as_str)
            .unwrap()
            .to_owned()
    });
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path))
            .await
            .unwrap();
        let instance = db.get_db("legacy").unwrap();
        let backing = instance.get_repo("main").unwrap().seed_backing_repo();
        let stores = backing.stores_list().await.unwrap();
        for name in ["__data__old", "__info__old", "__history__old"] {
            assert!(stores.iter().any(|stored| stored == name));
        }
        assert!(!stores.iter().any(|stored| {
            stored.starts_with("__data__old--")
                || stored.starts_with("__info__old--")
                || stored.starts_with("__history__old--")
        }));
        let renamed = instance.get_table("main", "renamed").await.unwrap();
        let expected = vec![
            (ids[0], InnerValue::Str("legacy-1".into())),
            (ids[1], InnerValue::Str("legacy-2".into())),
        ];
        assert_rows(&listed_rows(&renamed).await, &expected);
        assert_eq!(
            renamed
                .get_many(&ids)
                .await
                .unwrap()
                .into_iter()
                .map(|value| value.unwrap())
                .collect::<Vec<_>>(),
            vec![
                InnerValue::Str("legacy-1".into()),
                InnerValue::Str("legacy-2".into())
            ]
        );
        let row = db
            .system_store()
            .load_table_record("legacy", "main", "renamed")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.get("store_id").and_then(QueryValue::as_str),
            Some(renamed_store_id.as_str())
        );
    });
    drop(runtime);
}

#[test]
fn add_table_preserves_pre_registered_config_and_rows() {
    let dir = tempfile::tempdir().unwrap();
    let system_path = dir.path().join("system");
    let repo_path = dir.path().join("repo");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let cases = runtime.block_on(async {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
            .await
            .unwrap();
        shamir.create_db("p34").await.unwrap();
        shamir
            .add_repo(
                "p34",
                RepoConfig::new("main", BoxRepoFactory::fjall_raw(&repo_path)),
            )
            .await
            .unwrap();
        let repo = shamir.get_db("p34").unwrap().get_repo("main").unwrap();
        let mut cases = Vec::new();
        for (name, config) in [
            (
                "generated",
                TableConfig::new("generated")
                    .with_new_store_id()
                    .with_indexes(),
            ),
            ("legacy", TableConfig::new("legacy").with_indexes()),
        ] {
            let expected_id = config.store_id;
            let permit = repo.write_domain().admit().await.unwrap();
            repo.add_table_admitted(config, &permit).await.unwrap();
            drop(permit);
            let table = repo.get_table(name).await.unwrap();
            let ids = table
                .insert_many(&[InnerValue::Str(format!("{name}-row"))])
                .await
                .unwrap();
            let expected_rows = vec![(ids[0], InnerValue::Str(format!("{name}-row")))];
            shamir.add_table("p34", "main", name, false).await.unwrap();
            let actual = repo.table_config(name).unwrap();
            assert_eq!(actual.store_id, expected_id);
            assert!(actual.enable_indexes);
            let record = shamir
                .system_store()
                .load_table_record("p34", "main", name)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                record.get("store_id").and_then(QueryValue::as_str),
                expected_id.map(|id| id.to_string()).as_deref()
            );
            assert_eq!(
                record.get("enable_indexes").and_then(QueryValue::as_bool),
                Some(true)
            );
            assert_eq!(
                table
                    .get_many(&ids)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|value| value.unwrap())
                    .collect::<Vec<_>>(),
                vec![InnerValue::Str(format!("{name}-row"))]
            );
            cases.push((name.to_owned(), expected_id, ids, expected_rows));
        }
        shamir.flush_all_for_test().await.unwrap();
        cases
    });
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(system_path))
            .await
            .unwrap();
        let repo = shamir.get_db("p34").unwrap().get_repo("main").unwrap();
        for (name, expected_id, ids, expected_rows) in cases {
            let actual = repo.table_config(&name).unwrap();
            assert_eq!(actual.store_id, expected_id);
            assert!(actual.enable_indexes);
            let table = repo.get_table(&name).await.unwrap();
            assert_rows(&listed_rows(&table).await, &expected_rows);
            assert_eq!(
                table
                    .get_many(&ids)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|value| value.unwrap())
                    .collect::<Vec<_>>(),
                expected_rows
                    .iter()
                    .map(|(_, value)| value.clone())
                    .collect::<Vec<_>>()
            );
            let record = shamir
                .system_store()
                .load_table_record("p34", "main", &name)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                record.get("store_id").and_then(QueryValue::as_str),
                expected_id.map(|id| id.to_string()).as_deref()
            );
            assert_eq!(
                record.get("enable_indexes").and_then(QueryValue::as_bool),
                Some(true)
            );
        }
    });
    drop(runtime);
}

#[tokio::test]
async fn add_table_catalogue_failure_preserves_pre_registered_table() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("p34").await.unwrap();
    shamir
        .add_repo("p34", RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    let repo = shamir.get_db("p34").unwrap().get_repo("main").unwrap();
    let config = TableConfig::new("existing")
        .with_new_store_id()
        .with_indexes();
    let expected_id = config.store_id;
    let permit = repo.write_domain().admit().await.unwrap();
    repo.add_table_admitted(config, &permit).await.unwrap();
    drop(permit);
    let table = repo.get_table("existing").await.unwrap();
    let ids = table
        .insert_many(&[
            InnerValue::Str("keep-1".into()),
            InnerValue::Str("keep-2".into()),
        ])
        .await
        .unwrap();
    shamir.system_store().arm_catalogue_write_fault(1);
    assert!(shamir
        .add_table("p34", "main", "existing", false)
        .await
        .is_err());
    let actual = repo.table_config("existing").unwrap();
    assert_eq!(actual.store_id, expected_id);
    assert!(actual.enable_indexes);
    assert_eq!(
        table
            .get_many(&ids)
            .await
            .unwrap()
            .into_iter()
            .map(|value| value.unwrap())
            .collect::<Vec<_>>(),
        vec![
            InnerValue::Str("keep-1".into()),
            InnerValue::Str("keep-2".into())
        ]
    );
}
