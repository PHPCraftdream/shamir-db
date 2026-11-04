//! Read-only repo-wide walks (FK reverse-cache scan, shutdown flush) must
//! survive a table rename that ends "in doubt": the source name stays listed
//! by `list_table_names()` but is fenced (`get_table` returns `Conflict`),
//! so walkers resolve through the unfenced accessors instead.

use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_builder::write::UpdateReturnMode;
use shamir_query_types::admin::FkAction;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use smallvec::smallvec;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::query::batch::BatchError;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{
    build_reverse_fk_entries, BoxRepo, RenamePersistOutcome, RepoConfig, RepoInstance,
};
use crate::table::{TableConfig, TableManager};
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::foreign_key::ForeignKeyRef;
use crate::validator::schema::schema_validator::SchemaValidator;
use crate::validator::schema::type_tag::TypeTag;
use crate::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};

/// Resolver over a real `DbInstance` that mirrors `DbTableResolver`: the
/// fenced `resolve` for ordinary reads, plus an explicit `resolve_unfenced`
/// override for the read-only declaration walks (same validator-registry
/// injection on both paths).
struct WalkResolver {
    db: DbInstance,
    repo: String,
    registry: Arc<ValidatorRegistry>,
}

#[async_trait::async_trait]
impl TableResolver for WalkResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        let mut table = self.db.get_table(&self.repo, &table_ref.table).await?;
        table.set_validator_registry(Arc::clone(&self.registry));
        Ok(table)
    }

    async fn resolve_unfenced(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        let mut table = self
            .db
            .get_table_unfenced(&self.repo, &table_ref.table)
            .await?;
        table.set_validator_registry(Arc::clone(&self.registry));
        Ok(table)
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        self.db
            .get_repo(&self.repo)
            .ok_or_else(|| DbError::NotFound(format!("repo '{}' not found", self.repo)))
    }
}

/// Parent + child with a bound FK (`child.parent_id -> parent.id`), one
/// seeded parent row and one referencing child row. The child table is the
/// rename target: fencing it must not break parent writes whose FK
/// reverse-cache walk has to resolve the fenced child name.
async fn fixture() -> (WalkResolver, RepoInstance) {
    fixture_with_action(FkAction::NoAction).await
}

async fn fixture_with_action(action: FkAction) -> (WalkResolver, RepoInstance) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let repo = db.get_repo("default").unwrap();

    let registry = Arc::new(ValidatorRegistry::new());
    let child_schema = SchemaValidator::new(vec![FieldRule {
        path: vec!["parent_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_delete("parent", "id", action)),
            required: true,
            ..Default::default()
        },
        keyset_safe: false,
    }]);
    let validator_id = RecordId::from_ts(9501);
    registry
        .register(validator_id, "walk_child_fk_schema", Arc::new(child_schema))
        .unwrap();
    // Binding ops are irrelevant for `collect_fk_refs()` — it reads FK
    // metadata from the validator regardless (same reasoning as
    // fk_restrict_tests.rs).
    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };
    let mut child_table = db.get_table("default", "child").await.unwrap();
    child_table.set_validator_registry(Arc::clone(&registry));
    child_table.add_validator_binding(binding).await.unwrap();

    let resolver = WalkResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).unwrap().set("name", "P1").unwrap()),
    );
    let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.results["ins_parent"].records.len(), 1);

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins_child",
        write::insert("child").row(
            doc()
                .set("parent_id", 1)
                .unwrap()
                .set("label", "c1")
                .unwrap(),
        ),
    );
    let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.results["ins_child"].records.len(), 1);

    (resolver, repo)
}

/// Force the CHILD rename into the in-doubt (fenced) state using the same
/// `rename_table_stores_with(InDoubt)` pattern as `freeze_tests.rs`.
async fn fence_child_rename_in_doubt(repo: &RepoInstance) {
    assert!(matches!(
        repo.rename_table_stores_with("child", "child2", |_| async {
            RenamePersistOutcome::InDoubt(DbError::Storage("uncertain".into()))
        })
        .await,
        Err(DbError::Conflict(_))
    ));
    assert!(repo.has_rename_in_doubt());
    assert!(repo.check_table_rename_fence("child").is_err());
    assert!(matches!(
        repo.get_table("child").await,
        Err(DbError::Conflict(_))
    ));
}

#[tokio::test]
async fn parent_fk_update_still_works_while_rename_in_doubt() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let (resolver, repo) = fixture().await;
        fence_child_rename_in_doubt(&repo).await;

        // A parent update rebuilds the cold reverse-FK cache, whose repo-wide
        // walk must resolve the fenced child name.
        let mut b = Batch::new();
        b.id(3);
        b.try_update(
            "upd_parent",
            write::update("parent")
                .where_(filter::eq("id", 1))
                .set(doc().set("name", "P1-updated").unwrap())
                .returning(UpdateReturnMode::All),
        )
        .unwrap();
        let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
        assert_eq!(resp.results["upd_parent"].records.len(), 1);

        // Child FK bookkeeping still resolves: the read-only walk succeeds
        // and still discovers the child as a dependent of `parent`.
        let entries = build_reverse_fk_entries(&resolver, "default")
            .await
            .unwrap();
        assert!(entries.iter().any(|tagged| {
            tagged.parent_table == "parent" && tagged.entry.child_table == "child"
        }));
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn parent_fk_delete_still_works_while_rename_in_doubt() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let (resolver, repo) = fixture().await;
        fence_child_rename_in_doubt(&repo).await;

        // Parent delete triggers restrict/action discovery, whose repo-wide
        // reverse-cache walk must resolve the fenced child name. The FK is
        // NoAction, so the delete itself is allowed to proceed.
        let mut b = Batch::new();
        b.id(3);
        b.try_delete(
            "del_parent",
            write::delete("parent")
                .where_(filter::eq("id", 1))
                .returning(),
        )
        .unwrap();
        let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
        assert_eq!(resp.results["del_parent"].records.len(), 1);

        let entries = build_reverse_fk_entries(&resolver, "default")
            .await
            .unwrap();
        assert!(entries.iter().any(|tagged| {
            tagged.parent_table == "parent" && tagged.entry.child_table == "child"
        }));
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn fenced_child_restrict_parent_delete_fails_conflict() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let (resolver, repo) = fixture_with_action(FkAction::Restrict).await;
        fence_child_rename_in_doubt(&repo).await;

        // Fail closed: the RESTRICT gate resolves the fenced child through the
        // fenced accessor, so the parent delete surfaces the fence error
        // instead of silently skipping the child.
        let mut b = Batch::new();
        b.id(3);
        b.try_delete(
            "del_parent",
            write::delete("parent")
                .where_(filter::eq("id", 1))
                .returning(),
        )
        .unwrap();
        let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;
        assert!(matches!(
            resp,
            Err(BatchError::QueryError { code: Some(code), message, .. })
                if code == "fk_restrict" && message.contains("rename in doubt")
        ));
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn fenced_child_cascade_parent_delete_fails_conflict() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let (resolver, repo) = fixture_with_action(FkAction::Cascade).await;
        fence_child_rename_in_doubt(&repo).await;

        // Fail closed: the CASCADE action planner resolves the fenced child
        // through the fenced accessor, so the parent delete surfaces the
        // fence error instead of silently skipping the child.
        let mut b = Batch::new();
        b.id(3);
        b.try_delete(
            "del_parent",
            write::delete("parent")
                .where_(filter::eq("id", 1))
                .returning(),
        )
        .unwrap();
        let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;
        assert!(matches!(
            resp,
            Err(BatchError::QueryError { code: Some(code), message, .. })
                if code == "fk_actions" && message.contains("rename in doubt")
        ));
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn shutdown_flush_does_not_error_while_rename_in_doubt() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        // Same fixture shape as freeze_tests.rs: a bare RepoInstance whose
        // `flush_buffers` (shutdown flush) walks every listed table name.
        let repo = RepoInstance::new(
            "walk_flush_test".into(),
            BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
            vec![TableConfig::new("a")],
        );
        let _source = repo.get_table("a").await.unwrap();

        assert!(matches!(
            repo.rename_table_stores_with("a", "b", |_| async {
                RenamePersistOutcome::InDoubt(DbError::Storage("uncertain".into()))
            })
            .await,
            Err(DbError::Conflict(_))
        ));
        assert!(repo.has_rename_in_doubt());
        assert!(repo.check_table_rename_fence("a").is_err());

        // Shutdown flush only reads/flushes a stable frozen source, so it
        // must not fail on a fenced in-doubt name.
        repo.flush_buffers().await.unwrap();
    })
    .await
    .expect("test timed out");
}
