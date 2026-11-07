use crate::access::Actor;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::shamir_db::ShamirDb;
use crate::types::value::{InnerValue, QueryValue};
use futures::StreamExt;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::types::record_id::RecordId;

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

async fn setup(name: &str) -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db(name).await.unwrap();
    db.add_repo(name, RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    db.add_table(name, "main", "a", false).await.unwrap();
    db
}

#[tokio::test]
async fn wire_rename_table_reports_in_doubt_conflict_code_and_message() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let db = setup("wire-in-doubt").await;
        db.system_store().arm_durability_tail_faults(2);

        let mut batch = Batch::new();
        batch.id(1);
        batch.rename_table("rename", ddl::rename_table("a", "b").repo("main"));
        let request = batch
            .to_request_via_msgpack()
            .expect("batch request encoding");
        let error = db
            .execute_as(Actor::System, "wire-in-doubt", &request)
            .await
            .expect_err("in-doubt rename must be a query error");

        assert!(
            matches!(
                &error,
                crate::query::batch::BatchError::QueryError { code: Some(code), message, .. }
                    if code == "tx_conflict" && message.contains("rename in doubt:")
            ),
            "expected tx_conflict QueryError with rename-in-doubt message, got {error:?}"
        );
    })
    .await
    .expect("wire rename test exceeded 30 seconds");
}

#[tokio::test]
async fn wire_if_exists_ddl_cannot_bypass_in_doubt_rename_fence() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let db = setup("wire-fence-bypass").await;
        db.system_store().arm_durability_tail_faults(2);
        assert!(matches!(
            db.rename_table_as("wire-fence-bypass", "main", "a", "b", Actor::System).await,
            Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
        ));

        for name in ["a", "b"] {
            let mut create = Batch::new();
            create.id(1);
            create.op(
                "create",
                ddl::create_table(name).repo("main").if_not_exists(),
            );
            let create_request = create.to_request_via_msgpack().unwrap();
            let create_error = db
                .execute_as(Actor::System, "wire-fence-bypass", &create_request)
                .await
                .expect_err("CREATE IF NOT EXISTS must honor the rename fence");
            assert!(
                matches!(
                    &create_error,
                    crate::query::batch::BatchError::QueryError { code: Some(code), message, .. }
                        if code == "tx_conflict" && message.contains("rename in doubt:")
                ),
                "unexpected CREATE error: {create_error:?}"
            );

            let mut drop = Batch::new();
            drop.id(2);
            drop.op(
                "drop",
                ddl::drop_table(name)
                    .repo("main")
                    .if_exists()
                    .hmac("test-hmac")
                    .build()
                    .unwrap(),
            );
            let drop_request = drop.to_request_via_msgpack().unwrap();
            let drop_error = db
                .execute_as(Actor::System, "wire-fence-bypass", &drop_request)
                .await
                .expect_err("DROP IF EXISTS must honor the rename fence");
            assert!(
                matches!(
                    &drop_error,
                    crate::query::batch::BatchError::QueryError { code: Some(code), message, .. }
                        if code == "tx_conflict" && message.contains("rename in doubt:")
                ),
                "unexpected DROP error: {drop_error:?}"
            );
        }
    })
    .await
    .expect("wire fence-bypass test exceeded 30 seconds");
}

#[tokio::test]
async fn durability_tail_rolls_forward_and_is_idempotent() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let db = setup("roll-forward").await;
        let repo = db.get_db("roll-forward").unwrap().get_repo("main").unwrap();
        let source = repo.get_table("a").await.unwrap();
        let ids = source
            .insert_many(&[InnerValue::Str("one".into()), InnerValue::Str("two".into())])
            .await
            .unwrap();
        let expected = vec![
            (ids[0], InnerValue::Str("one".into())),
            (ids[1], InnerValue::Str("two".into())),
        ];
        db.system_store().arm_durability_tail_faults(1);
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            db.rename_table_as("roll-forward", "main", "a", "b", Actor::System),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!repo.has_table("a"));
        assert!(repo.has_table("b"));
        assert_exact_rows(&rows(&repo.get_table("b").await.unwrap()).await, &expected);
        db.rename_table_as("roll-forward", "main", "a", "b", Actor::System)
            .await
            .unwrap();
        assert_exact_rows(&rows(&repo.get_table("b").await.unwrap()).await, &expected);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn in_doubt_fences_name_token_ddl_and_retry_converges() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
    let db = setup("in-doubt").await;
    let repo = db.get_db("in-doubt").unwrap().get_repo("main").unwrap();
    let source = repo.get_table("a").await.unwrap();
    let ids = source.insert_many(&[InnerValue::Str("kept".into())]).await.unwrap();
    let expected = vec![(ids[0], InnerValue::Str("kept".into()))];
    let source_id = source.table_token();
    db.system_store().arm_durability_tail_faults(2);
    let result = db.rename_table_as("in-doubt", "main", "a", "b", Actor::System).await;
    assert!(matches!(result, Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")));
    assert!(repo.has_rename_in_doubt());
    for name in ["a", "b"] {
        assert!(matches!(repo.get_table(name).await, Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")));
        assert!(matches!(db.add_table("in-doubt", "main", name, false).await, Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")));
        assert!(matches!(db.drop_table("in-doubt", "main", name).await, Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")));
    }
    assert!(matches!(
        db.rename_table_as("in-doubt", "main", "a", "c", Actor::System).await,
        Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
    ));
    assert!(matches!(
        db.rename_repo_as("in-doubt", "main", "other", Actor::System).await,
        Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
    ));
    assert!(matches!(
        repo.table_by_token(source_id).await.unwrap().unwrap().insert(&InnerValue::Str("stale".into())).await,
        Err(crate::DbError::MigrationFrozen { table }) if table == "a"
    ));
    assert!(matches!(
        source.create_index("stale-index", &["name"]).await,
        Err(crate::DbError::MigrationFrozen { table }) if table == "a"
    ));
    let mut expected_catalogue_id = None;
    for name in ["a", "b"] {
        if let Some(row) = db.system_store().load_table_record("in-doubt", "main", name).await.unwrap() {
            assert!(expected_catalogue_id.replace(row.get("store_id").and_then(QueryValue::as_str).unwrap().to_owned()).is_none());
        }
    }
    assert!(expected_catalogue_id.is_some());
    db.rename_table_as("in-doubt", "main", "a", "b", Actor::System).await.unwrap();
    assert!(!repo.has_rename_in_doubt());
    assert!(!repo.has_table("a"));
    assert!(repo.has_table("b"));
    let renamed = repo.get_table("b").await.unwrap();
    assert_exact_rows(&rows(&renamed).await, &expected);
    db.rename_table_as("in-doubt", "main", "a", "b", Actor::System).await.unwrap();
    assert_exact_rows(&rows(&renamed).await, &expected);
    }).await.unwrap();
}

#[tokio::test]
async fn recovery_durability_failure_retains_fence_until_retry() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let db = setup("recovery-durability").await;
        let repo = db
            .get_db("recovery-durability")
            .unwrap()
            .get_repo("main")
            .unwrap();
        let source = repo.get_table("a").await.unwrap();
        let ids = source
            .insert_many(&[InnerValue::Str("durable".into())])
            .await
            .unwrap();
        let expected = vec![(ids[0], InnerValue::Str("durable".into()))];

        db.system_store().arm_durability_tail_faults(2);
        assert!(matches!(
            db.rename_table_as("recovery-durability", "main", "a", "b", Actor::System).await,
            Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
        ));
        db.system_store().arm_durability_tail_faults(2);
        assert!(matches!(
            db.rename_table_as("recovery-durability", "main", "a", "b", Actor::System).await,
            Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
        ));

        assert!(repo.has_rename_in_doubt());
        assert!(repo.has_table("a") || repo.has_table("b"));
        for name in ["a", "b"] {
            assert!(matches!(
                repo.get_table(name).await,
                Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
            ));
        }
        assert!(matches!(
            source.create_index("stale-index", &["name"]).await,
            Err(crate::DbError::MigrationFrozen { table }) if table == "a"
        ));

        db.rename_table_as("recovery-durability", "main", "a", "b", Actor::System)
            .await
            .unwrap();
        assert!(!repo.has_rename_in_doubt());
        assert!(!repo.has_table("a"));
        assert!(repo.has_table("b"));
        assert_exact_rows(&rows(&repo.get_table("b").await.unwrap()).await, &expected);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn retry_precommit_fault_keeps_existing_fence() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let db = setup("retry-fault").await;
        let repo = db.get_db("retry-fault").unwrap().get_repo("main").unwrap();
        db.system_store().arm_durability_tail_faults(2);
        assert!(db
            .rename_table_as("retry-fault", "main", "a", "b", Actor::System)
            .await
            .is_err());
        db.system_store().arm_catalogue_write_fault(1);
        assert!(matches!(
            db.rename_table_as("retry-fault", "main", "a", "b", Actor::System).await,
            Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
        ));
        assert!(repo.has_rename_in_doubt());
        db.rename_table_as("retry-fault", "main", "a", "b", Actor::System)
            .await
            .unwrap();
        assert!(!repo.has_rename_in_doubt());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn precommit_failure_leaves_source_writable_and_uses_fresh_id() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let db = setup("precommit").await;
        let repo = db.get_db("precommit").unwrap().get_repo("main").unwrap();
        let source = repo.get_table("a").await.unwrap();
        db.system_store().arm_catalogue_write_fault(1);
        assert!(db
            .rename_table_as("precommit", "main", "a", "b", Actor::System)
            .await
            .is_err());
        let added = source
            .insert(&InnerValue::Str("still writable".into()))
            .await
            .unwrap();
        assert!(repo.has_table("a"));
        assert!(!repo.has_table("b"));
        assert!(db
            .system_store()
            .load_table_record("precommit", "main", "b")
            .await
            .unwrap()
            .is_none());
        db.rename_table_as("precommit", "main", "a", "b", Actor::System)
            .await
            .unwrap();
        assert!(rows(&repo.get_table("b").await.unwrap())
            .await
            .iter()
            .any(|(id, value)| *id == added && value == &InnerValue::Str("still writable".into())));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn overlong_rename_destination_fails_before_persistence() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let table_name = "x".repeat(255);
        let db = setup("too-long").await;
        let source = db.get_table("too-long", "main", "a").await.unwrap();
        let result = db
            .rename_table_as("too-long", "main", "a", &table_name, Actor::System)
            .await;
        assert!(matches!(result, Err(crate::DbError::Validation(_))));
        assert!(source
            .insert(&InnerValue::Str("writable".into()))
            .await
            .is_ok());
        assert!(db
            .system_store()
            .load_table_record("too-long", "main", &table_name)
            .await
            .unwrap()
            .is_none());
    })
    .await
    .unwrap();
}
