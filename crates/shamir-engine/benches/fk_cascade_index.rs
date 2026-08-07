//! F-53c — FK CASCADE / SET NULL index-fast-path vs scan-fallback bench.
//!
//! Proves the F-53c fix: when a supporting single-field index exists on the
//! FK's referencing column, CASCADE/SET NULL actions gather candidate child
//! rows via `lookup_by_index` (O(|matches|)) instead of an unconditional
//! `list_stream_tx` full-table scan (O(N)). The sibling RESTRICT action
//! (`fk_restrict::child_has_reference`) already used this index fast-path;
//! this fix wires the same pattern into `fk_actions` (CASCADE/SET NULL on
//! DELETE) and `fk_on_update` (CASCADE/SET NULL on UPDATE).
//!
//! ## Workload
//!
//! A parent table with two rows and a child table of `N` rows, where only a
//! small fraction (`MATCH_FRACTION`, ~0.5% at N=10k → ~50 rows) reference the
//! parent row being cascade-deleted; the rest reference a survivor parent and
//! must be scanned past by the no-index path. Two registered workloads share
//! the IDENTICAL data shape and cascade operation — they differ only in
//! whether a supporting index exists on `child.parent_id`:
//!
//! - `cascade_scan_no_index/{N}` — NO index → the scan fallback streams +
//!   matches all `N` child rows (the pre-F-53c behavior, always paid).
//! - `cascade_index_fast_path/{N}` — supporting index → the F-53c fast-path
//!   fetches only the ~`MATCH_FRACTION * N` matching RecordIds directly.
//!
//! Both produce the IDENTICAL result (same children cascaded); the bench
//! measures the planning-scan cost difference. The scan is O(N) in the child
//! table size; the index path is O(|matches|) — so the gap widens as N grows.
//!
//! Uses the fixed-iteration harness (`bench_scale_tool`) with `bench_batched`:
//! CASCADE mutates the child table, so the fixture is rebuilt fresh (untimed)
//! before every iteration and only the `execute_batch` cascade routine is
//! timed. Child seeding goes through the fast `TableManager::insert_many`
//! bulk path (the cascade machinery is triggered by the DELETE through
//! `execute_batch`, which is what's measured).
//!
//! Run: `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p
//! shamir-engine --bench fk_cascade_index`

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::db_instance::db_instance::DbInstance;
use shamir_engine::query::batch::{execute_batch, TableResolver};
use shamir_engine::query::TableRef;
use shamir_engine::repo::{BoxRepoFactory, RepoConfig, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_engine::validator::schema::constraints::Constraints;
use shamir_engine::validator::schema::field_rule::FieldRule;
use shamir_engine::validator::schema::foreign_key::ForeignKeyRef;
use shamir_engine::validator::schema::schema_validator::SchemaValidator;
use shamir_engine::validator::schema::type_tag::TypeTag;
use shamir_engine::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::smallvec;

/// Child-table sizes. The bench's point is to watch the scan-path cost grow
/// roughly linearly with N while the index-fast-path cost stays ~flat (tied
/// to |matches|, not N). Default tiers keep each iteration's rebuild bounded;
/// larger N (where the O(N) cliff is dramatic) is opt-in.
fn child_sizes() -> Vec<usize> {
    let mut sizes = vec![5_000, 10_000];
    if parse_bool_env("BENCH_FK_CASCADE_HUGE") {
        sizes.push(50_000);
    }
    sizes
}

fn parse_bool_env(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Fraction of child rows that reference the cascade-target parent (the rest
/// reference a DIFFERENT parent that survives, so they must be scanned past
/// by the no-index path but skipped by the index path). ~0.5% at N=10k → ~50
/// matches; large enough to be realistic, small enough that the O(N) scan
/// dominates the no-index path.
const MATCH_FRACTION: f64 = 0.005;

struct FkBenchResolver {
    repo: RepoInstance,
    registry: Arc<ValidatorRegistry>,
}

#[async_trait::async_trait]
impl TableResolver for FkBenchResolver {
    async fn resolve(&self, table_ref: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        let mut table = self.repo.get_table(&table_ref.table).await?;
        table.set_validator_registry(Arc::clone(&self.registry));
        Ok(table)
    }

    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

/// Build the fixture: parent rows {1, 2}; `n` child rows where the first
/// `ceil(n * MATCH_FRACTION)` reference parent 1 (cascade target) and the
/// rest reference parent 2 (survivor). When `with_index` is true, a
/// single-field index on `child.parent_id` is created so the F-53c fast-path
/// engages; otherwise the scan fallback runs.
async fn build_fixture(n: usize, with_index: bool) -> FkBenchResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config])
        .await
        .expect("with_repos");
    let repo = db.get_repo("default").expect("get_repo");

    // Bind a CASCADE FK on child.parent_id → parent.id so the DELETE below
    // triggers `plan_cascade` (the scan-heavy path this fix targets).
    let registry = Arc::new(ValidatorRegistry::new());
    let child_schema = SchemaValidator::new(vec![FieldRule {
        path: vec!["parent_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_delete(
                "parent",
                "id",
                FkAction::Cascade,
            )),
            required: false,
            nullable: true,
            ..Default::default()
        },
        keyset_safe: false,
    }]);
    let validator_id = RecordId::from_ts(9401);
    registry
        .register(
            validator_id,
            "bench_child_fk_cascade",
            Arc::new(child_schema),
        )
        .expect("register validator");
    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };
    let mut child_table = db
        .get_table("default", "child")
        .await
        .expect("get_table child");
    child_table.set_validator_registry(Arc::clone(&registry));
    child_table
        .add_validator_binding(binding)
        .await
        .expect("add_validator_binding");

    let resolver = FkBenchResolver {
        repo: repo.clone(),
        registry: Arc::clone(&registry),
    };

    // Seed two parents through the batch path (only 2 rows — cheap).
    let mut seed = Batch::new();
    seed.id(1);
    seed.insert(
        "p1",
        write::insert("parent").row(doc().set("id", 1_i64).set("name", "cascade_target")),
    );
    seed.insert(
        "p2",
        write::insert("parent").row(doc().set("id", 2_i64).set("name", "survivor")),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "bench")
        .await
        .expect("seed parents");

    // Seed `n` children via the fast bulk `insert_many` path (bypassing the
    // per-row batch/validator overhead — only the row POPULATION matters for
    // the cascade scan; the FK is discovered from the validator bound above).
    let match_count = (n as f64 * MATCH_FRACTION).ceil() as usize;
    let interner = child_table.interner().get().await.expect("interner");
    let k_parent_id = match interner.touch_ind("parent_id").expect("touch parent_id") {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let k_cid = match interner.touch_ind("cid").expect("touch cid") {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };

    let chunk = 4_000;
    let mut batch: Vec<InnerValue> = Vec::with_capacity(chunk);
    for i in 0..n {
        let parent_id = if i < match_count { 1_i64 } else { 2_i64 };
        let mut m = new_map_wc(2);
        m.insert(k_cid.clone(), InnerValue::Int(i as i64));
        m.insert(k_parent_id.clone(), InnerValue::Int(parent_id));
        batch.push(InnerValue::Map(m));
        if batch.len() >= chunk || i == n - 1 {
            child_table
                .insert_many(&batch)
                .await
                .expect("insert_many children");
            batch.clear();
        }
    }

    if with_index {
        child_table
            .create_index("idx_child_parent_id", &["parent_id"])
            .await
            .expect("create index");
    }

    resolver
}

/// One CASCADE delete of parent id=1 — the F-53c planning scan runs here.
fn cascade_delete_batch() -> shamir_query_types::batch::BatchRequest {
    let mut b = Batch::new();
    b.id(999);
    b.try_delete(
        "del",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    b.build()
}

fn main() {
    let mut h = Harness::new("fk_cascade_index", env!("CARGO_MANIFEST_DIR"));

    for &n in &child_sizes() {
        let match_count = (n as f64 * MATCH_FRACTION).ceil() as usize;
        eprintln!(
            "fk_cascade_index: N={n}, cascade matches≈{match_count} ({:.2}% of N)",
            (match_count as f64 / n as f64) * 100.0
        );

        // No-index scan fallback (the pre-F-53c behavior, always paid):
        // fresh fixture per iteration, only the cascade-delete routine timed.
        let n_scan = n;
        h.bench_batched(
            &format!("cascade_scan_no_index/{n}"),
            move || {
                let runtime = rt();
                let resolver = runtime.block_on(build_fixture(n_scan, false));
                (runtime, resolver)
            },
            |(runtime, resolver)| {
                let req = cascade_delete_batch();
                runtime
                    .block_on(execute_batch(
                        &req,
                        &resolver,
                        None,
                        None,
                        Actor::System,
                        "bench",
                    ))
                    .expect("cascade delete (scan path)");
            },
        );

        // F-53c index fast-path: same data shape + operation, but a
        // supporting index routes the planning scan through
        // `lookup_by_index` (O(|matches|)) instead of a full stream.
        let n_idx = n;
        h.bench_batched(
            &format!("cascade_index_fast_path/{n}"),
            move || {
                let runtime = rt();
                let resolver = runtime.block_on(build_fixture(n_idx, true));
                (runtime, resolver)
            },
            |(runtime, resolver)| {
                let req = cascade_delete_batch();
                runtime
                    .block_on(execute_batch(
                        &req,
                        &resolver,
                        None,
                        None,
                        Actor::System,
                        "bench",
                    ))
                    .expect("cascade delete (index path)");
            },
        );
    }

    h.run();
}
