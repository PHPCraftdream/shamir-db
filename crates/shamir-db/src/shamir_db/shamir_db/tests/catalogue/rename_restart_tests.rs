use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::shamir_db::{ShamirDb, SystemStoreConfig};
use crate::types::value::InnerValue;
use futures::StreamExt;
use shamir_types::types::record_id::RecordId;
use tokio::runtime::Builder;

async fn rows(table: &crate::engine::table::TableManager) -> Vec<(RecordId, InnerValue)> {
    table
        .list_stream(32)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flat_map(Result::unwrap)
        .map(|(id, value)| (id, value.into_inner().unwrap()))
        .collect()
}

fn assert_exact_rows(actual: &[(RecordId, InnerValue)], expected: &[(RecordId, InnerValue)]) {
    assert_eq!(actual.len(), expected.len());
    for row in expected {
        assert!(actual.contains(row));
    }
}

#[test]
fn in_doubt_rename_reopens_as_exactly_one_catalogued_incarnation() {
    let directory = tempfile::tempdir().unwrap();
    let system_path = directory.path().join("system");
    let repo_path = directory.path().join("repo");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let (expected_rows, old_store_id, new_store_id) = runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
                .await
                .unwrap();
            db.create_db("restart").await.unwrap();
            db.add_repo(
                "restart",
                RepoConfig::new("main", BoxRepoFactory::fjall(&repo_path)),
            )
            .await
            .unwrap();
            db.add_table("restart", "main", "a", false).await.unwrap();
            let repo = db.get_db("restart").unwrap().get_repo("main").unwrap();
            let source = repo.get_table("a").await.unwrap();
            let ids = source
                .insert_many(&[
                    InnerValue::Str("persisted-one".into()),
                    InnerValue::Str("persisted-two".into()),
                ])
                .await
                .unwrap();
            let expected = vec![
                (ids[0], InnerValue::Str("persisted-one".into())),
                (ids[1], InnerValue::Str("persisted-two".into())),
            ];
            let source_record = db
                .system_store()
                .load_table_record("restart", "main", "a")
                .await
                .unwrap()
                .unwrap();
            let old_id = source_record
                .get("store_id")
                .and_then(crate::types::value::QueryValue::as_str)
                .unwrap()
                .to_owned();
            db.system_store().arm_durability_tail_faults(2);
            assert!(matches!(
                db.rename_table_as("restart", "main", "a", "b", crate::access::Actor::System).await,
                Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
            ));
            assert!(repo.has_rename_in_doubt());
            let destination_record = db
                .system_store()
                .load_table_record("restart", "main", "b")
                .await
                .unwrap()
                .unwrap();
            let new_id = destination_record
                .get("store_id")
                .and_then(crate::types::value::QueryValue::as_str)
                .unwrap()
                .to_owned();
            (expected, old_id, new_id)
        })
        .await
        .unwrap()
    });
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path))
                .await
                .unwrap();
            let repo = db.get_db("restart").unwrap().get_repo("main").unwrap();
            assert!(!repo.has_rename_in_doubt());
            let a_row = db
                .system_store()
                .load_table_record("restart", "main", "a")
                .await
                .unwrap();
            let b_row = db
                .system_store()
                .load_table_record("restart", "main", "b")
                .await
                .unwrap();
            assert_ne!(a_row.is_some(), b_row.is_some());
            let (name, row, other) = if let Some(row) = b_row {
                ("b", row, "a")
            } else {
                ("a", a_row.unwrap(), "b")
            };
            assert!(repo.has_table(name));
            assert!(!repo.has_table(other));
            let store_id = row
                .get("store_id")
                .and_then(crate::types::value::QueryValue::as_str)
                .unwrap();
            let expected_name = if store_id == old_store_id {
                "a"
            } else {
                assert_eq!(store_id, new_store_id);
                "b"
            };
            assert_eq!(name, expected_name);
            let table = repo.get_table(name).await.unwrap();
            assert_exact_rows(&rows(&table).await, &expected_rows);
            assert!(db
                .system_store()
                .load_table_record("restart", "main", other)
                .await
                .unwrap()
                .is_none());
            db.add_table("restart", "main", other, false).await.unwrap();
            assert!(repo.has_table(other));
        })
        .await
        .unwrap()
    });
    drop(runtime);
}
