use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::db_instance::db_instance::DbInstance;
use shamir_engine::query::batch::TableResolver;
use shamir_engine::query::TableRef;
use shamir_engine::repo::{BoxRepoFactory, RepoConfig, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_engine::validator::schema::{
    Constraints, FieldRule, ForeignKeyRef, SchemaValidator, TypeTag,
};
use shamir_engine::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use smallvec::smallvec;

use crate::execute_batch;

pub(in super::super) fn run(h: &mut Harness) {
    // --- Group 9: F-46 concurrency impact — FK-child disjoint inserts -------
    //
    // Same disjoint-key/N-writer/K-row shape as Group 1, but the target
    // table has a REQUIRED foreign key, so every writer's insert runs
    // `require_footprint_if_fk_child` and gets `footprint_tokens` populated
    // — which F-46 widened `commit_tx_lockfree`'s commit-lock condition to
    // key off. Driven via `execute_batch` (not raw `insert_tx`) because the
    // FK-child hook lives in `query_runner.rs`, one layer above the table
    // API Group 1 drives directly.
    async fn make_fk_child_repo() -> RepoInstance {
        let repo_config = RepoConfig {
            name: "default".to_string(),
            factory: BoxRepoFactory::in_memory(),
            tables: vec![
                TableConfig::new("bench_parent"),
                TableConfig::new("bench_child"),
            ],
        };
        let db = DbInstance::with_repos(vec![repo_config])
            .await
            .expect("with_repos");
        let repo = db.get_repo("default").expect("get_repo");

        // A required FK on `bench_child.parent_id` → `bench_parent.id`, same
        // shape `fk_ri_barrier_tests.rs::setup_race_test` uses. The on_delete
        // action is irrelevant here (no delete traffic in this bench) — only
        // the FK's PRESENCE matters, since `FkReverseCache::is_fk_child`
        // (which `require_footprint_if_fk_child` consults) is keyed on the
        // constraint existing, not on which action it declares.
        let registry = Arc::new(ValidatorRegistry::new());
        let child_schema = SchemaValidator::new(vec![FieldRule {
            path: vec!["parent_id".to_string()],
            ty: TypeTag::Int,
            constraints: Constraints {
                foreign_key: Some(ForeignKeyRef::with_on_delete(
                    "bench_parent",
                    "id",
                    FkAction::Restrict,
                )),
                required: true,
                ..Default::default()
            },
            keyset_safe: false,
        }]);
        let validator_id = RecordId::from_ts(9201);
        registry
            .register(
                validator_id,
                "bench_child_fk_schema",
                Arc::new(child_schema),
            )
            .expect("register validator");
        let binding = ValidatorBinding {
            validator_id,
            ops: smallvec![WriteOp::Delete],
            priority: 1000,
        };
        let mut child_table = db
            .get_table("default", "bench_child")
            .await
            .expect("get_table bench_child");
        child_table.set_validator_registry(Arc::clone(&registry));
        child_table
            .add_validator_binding(binding)
            .await
            .expect("add_validator_binding");

        // Seed one parent row so the FK is always satisfiable — this bench
        // measures commit-lock contention cost, not FK-violation aborts.
        let mut seed = Batch::new();
        seed.id(1);
        seed.insert(
            "ins_parent",
            write::insert("bench_parent")
                .row(doc().set("id", 1).unwrap().set("name", "seed").unwrap()),
        );
        let resolver = FkChildBenchResolver { repo: repo.clone() };
        execute_batch(&seed.build(), &resolver, None, None, Actor::System, "bench")
            .await
            .expect("seed parent row");

        repo
    }

    struct FkChildBenchResolver {
        repo: RepoInstance,
    }

    #[async_trait::async_trait]
    impl TableResolver for FkChildBenchResolver {
        async fn resolve(
            &self,
            table_ref: &TableRef,
        ) -> shamir_storage::error::DbResult<TableManager> {
            self.repo.get_table(&table_ref.table).await
        }
        async fn resolve_repo(
            &self,
            _repo_name: &str,
        ) -> shamir_storage::error::DbResult<RepoInstance> {
            Ok(self.repo.clone())
        }
    }

    const FK_BENCH_ROWS_PER_WRITER: usize = 10;
    let fk_iter_ctr = Arc::new(AtomicU64::new(0));

    // n=1 matches Group 1's registered ladder point exactly (uncontended
    // floor). n=2/4/8 are ADDITIONAL points specific to this group — added
    // deliberately (unlike Group 1's collapsed-to-n=1 ladder) because F-46's
    // concurrency-impact question is precisely about what happens as N
    // grows once every writer takes the SAME `commit_lock`.
    for &n in &[1usize, 2, 4, 8] {
        let fk_iter_ctr = Arc::clone(&fk_iter_ctr);
        h.bench_batched_async(
            &format!("fk_child_disjoint_inserts/n_{n}"),
            move || {
                let iter_i = fk_iter_ctr.fetch_add(1, Ordering::Relaxed);
                async move {
                    let repo = make_fk_child_repo().await;
                    (repo, iter_i)
                }
            },
            move |(repo, iter_i)| async move {
                let mut handles = Vec::with_capacity(n);
                for w in 0..n {
                    let resolver = FkChildBenchResolver { repo: repo.clone() };
                    let base = (iter_i as usize * n + w) * FK_BENCH_ROWS_PER_WRITER;
                    handles.push(tokio::spawn(async move {
                        let mut b = Batch::new();
                        b.id("w");
                        for i in 0..FK_BENCH_ROWS_PER_WRITER {
                            b.insert(
                                format!("ins_{i}"),
                                write::insert("bench_child").row(
                                    doc()
                                        .set("parent_id", 1)
                                        .unwrap()
                                        .set("label", format!("v_{}_{}", base, i))
                                        .unwrap(),
                                ),
                            );
                        }
                        let out = execute_batch(
                            &b.build(),
                            &resolver,
                            None,
                            None,
                            Actor::System,
                            "bench",
                        )
                        .await
                        .unwrap();
                        std::hint::black_box(out);
                    }));
                }
                for hd in handles {
                    hd.await.unwrap();
                }
            },
        );
    }
}
