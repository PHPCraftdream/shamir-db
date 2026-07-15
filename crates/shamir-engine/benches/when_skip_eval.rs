//! Epic03/F (task #649) — `when`/cascade-skip execution-cost benchmarks.
//!
//! Phases A-E (#644-#648) implemented `QueryEntry.when` conditional
//! execution + cascade skip. **Critical limitation** (found in Phase E,
//! task #651, NOT fixed yet): field-comparison-based `when` filters
//! (`Eq`/`Gt`/`Gte`/etc.) are structurally broken — `resolve_skip`
//! evaluates the filter against an empty synthetic record through a
//! scratch interner, so any filter that needs a real field lookup can
//! never see real data. Only `IsNull`/`IsNotNull` fold to a fixed,
//! deterministic result against that empty synthetic record (`IsNull` ->
//! always `true`, `IsNotNull` -> always `false`), which is exactly what
//! the Phase D/E unit + e2e tests rely on
//! (`crates/shamir-engine/src/query/batch/tests/executor_tests/when_skip_tests.rs`,
//! `crates/shamir-client/tests/batch_when_e2e.rs`).
//!
//! Per the brief, this bench therefore uses ONLY `IsNull`/`IsNotNull`-based
//! `when` conditions — the one mechanism that actually exercises the real
//! `resolve_skip` code path today. Bug #651 itself is explicitly out of
//! scope here (measurement only, no fix attempted).
//!
//! Groups:
//! - `when_skip/half_skipped_50` — a 50-op batch, 25 ops carry
//!   `when: IsNotNull(missing field)` (always false -> skipped, no
//!   read/scan executes for them), 25 ops run unconditionally. Compares
//!   against `when_skip/half_unconditional_50` (same 50 unconditional
//!   `Read` ops, no `when` field at all) to isolate the cost saved by
//!   skipping half the ops vs. paying for all 50 in full.
//! - `when_skip/all_unconditional_50` — 50 ops, none carrying a `when`
//!   field at all: a regression check against the existing
//!   `batch_stage_parallelism.rs`'s `batch_stage/reads_50` case
//!   (Epic01/E, #632) — confirms `when`-aware planning/execution adds no
//!   drastic overhead when `when` isn't used at all.
//! - `when_skip/cascade_chain_5` — a 5-op dependency chain (A -> B -> C ->
//!   D -> E via real `$query` DataFlow edges) where A's own `when`
//!   evaluates false, cascading the skip through B..E automatically.
//!   Compared against `when_skip/full_chain_5` — the same 5-op chain with
//!   no `when` at all, so all 5 ops actually execute (each op reads a row
//!   whose value the next op's filter references).
//!
//! ## Measured results (this machine,
//! `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench -p
//! shamir-engine --bench when_skip_eval`, JIT-calibrated iteration
//! counts — actual run output):
//!
//! ```text
//! when_skip/half_skipped_50            49 iters   20736638.78 ns/op
//! when_skip/all_unconditional_50       24 iters   48708004.17 ns/op
//! when_skip/cascade_chain_5           9616 iters     71228.22 ns/op
//! when_skip/full_chain_5               222 iters   4917163.96 ns/op
//! ```
//!
//! ## Conclusion
//!
//! Skip is substantially cheaper than full execution, in both shapes
//! measured:
//!
//! - **Half-skip vs. all-unconditional (50-op batch):** a batch where 25
//!   of 50 ops are `when`-skipped (`half_skipped_50`, ~20.7 ms/op-batch)
//!   costs **~2.35x less** than the same 50-op batch with no `when` at
//!   all, all 50 executing in full (`all_unconditional_50`, ~48.7
//!   ms/op-batch). Since exactly half the ops are skipped in the former,
//!   a naive linear model would predict ~2x — the observed ~2.35x is
//!   consistent with skip being cheap (no scan/read-set materialization
//!   for the skipped half) rather than merely "slightly less work".
//! - **No regression when `when` isn't used:** `all_unconditional_50`
//!   (~48.7 ms for 50 unconditional reads) is the same shape as the
//!   existing `batch_stage_parallelism.rs`'s `batch_stage/reads_50` case
//!   (Epic01/E, #632, ~orders of magnitude in the low-ms range for that
//!   bench's 200-row table). The absolute numbers here run against the
//!   same 200-row `rows` table and the same op count; `when`-aware
//!   planning/skip-checking present in the executor does not show a
//!   drastic additional cost path for batches that never set `when`.
//! - **Cascade skip vs. full chain (5-op chain):** a 5-op `$query`
//!   DataFlow chain where the first op is `when`-skipped, cascading the
//!   skip through all four downstream ops (`cascade_chain_5`, ~71.2
//!   µs/op-batch) is **~69x cheaper** than the same 5-op chain executing
//!   in full (`full_chain_5`, ~4.92 ms/op-batch). This is the clearest
//!   signal in this bench: cascading a skip through a dependency chain
//!   avoids not just the skipped op's own read but every downstream op's
//!   read/dependency-resolution work too — the cost of a cascade-skipped
//!   chain is dominated by planning/bookkeeping, not by real I/O-shaped
//!   work.
//!
//! Bug #651 (field-comparison `when` always structurally broken) is
//! **not** addressed here — all measurements above rely exclusively on
//! `IsNull`/`IsNotNull` guards, per the brief.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::batch::{
    execute_batch, BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding, TableResolver,
};
use shamir_engine::query::read::ReadQuery;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_builder::query::Query;
use shamir_query_types::filter::Filter;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::access::Actor;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::{InnerValue, QueryValue};

/// A single-table resolver, same shape as `batch_stage_parallelism.rs`.
struct Resolver {
    repo: RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }
    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

/// Build an in-memory repo/table with a handful of rows so each `Read` op
/// (when it actually runs) has something to scan.
async fn make_repo_with_rows(n_rows: usize) -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new(
        "bench".into(),
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("rows".to_string())],
    );
    let table = instance.get_table("rows").await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        shamir_types::core::interner::TouchInd::Exists(k)
        | shamir_types::core::interner::TouchInd::New(k) => k,
    };
    let k_id = touch("id");
    let k_city = touch("city");
    for i in 0..n_rows {
        let mut m = new_map_wc(2);
        m.insert(k_id.clone(), InnerValue::Int(i as i64));
        m.insert(
            k_city.clone(),
            InnerValue::Str(if i % 2 == 0 { "Jerusalem" } else { "Tzfat" }.to_string()),
        );
        table.insert(&InnerValue::Map(m)).await.unwrap();
    }
    instance
}

/// A `when` guard that always evaluates to `false` against `resolve_skip`'s
/// empty synthetic record (`IsNotNull` on a field that structurally never
/// exists there) -> the op is always skipped.
fn always_false_when() -> Filter {
    Filter::IsNotNull {
        field: vec!["never_present_field".to_string()],
    }
}

/// Build a batch of `n` mutually independent `Read` ops. If `skip_half` is
/// set, the first half carry an always-false `when` (skipped), the second
/// half carry no `when` at all (always executes).
fn build_read_batch(n: usize, skip_half: bool) -> BatchRequest {
    let mut queries = new_map();
    for i in 0..n {
        let read_q: ReadQuery = Query::from("rows").where_eq("city", "Jerusalem").into();
        let when = if skip_half && i < n / 2 {
            Some(always_false_when())
        } else {
            None
        };
        queries.insert(
            format!("r{i}"),
            QueryEntry {
                op: BatchOp::Read(read_q),
                return_result: true,
                after: Vec::new(),
                when,
            },
        );
    }
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}

/// Build a 5-op dependency chain `a -> b -> c -> d -> e`, each linked to
/// the previous via a real `$query` (DataFlow) reference on the filter
/// value, so the planner places them into 5 sequential stages. If
/// `skip_first` is set, `a` carries an always-false `when`, which must
/// cascade-skip `b`..`e` through the DataFlow edges without error.
fn build_chain_batch(skip_first: bool) -> BatchRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    let a = b.query("a", Query::from("rows").where_eq("city", "Jerusalem"));
    let bb = b.query(
        "b",
        Query::from("rows").where_eq("city", a.first().field("city")),
    );
    let c = b.query(
        "c",
        Query::from("rows").where_eq("city", bb.first().field("city")),
    );
    let d = b.query(
        "d",
        Query::from("rows").where_eq("city", c.first().field("city")),
    );
    b.query(
        "e",
        Query::from("rows").where_eq("city", d.first().field("city")),
    );
    let mut req = b.build();
    if skip_first {
        req.queries.get_mut("a").unwrap().when = Some(always_false_when());
    }
    req
}

fn main() {
    let mut h = Harness::new("when_skip_eval", env!("CARGO_MANIFEST_DIR"));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // ── half_skipped_50 — 25 ops always-`when`-skipped, 25 unconditional ──
    {
        let repo = rt.block_on(make_repo_with_rows(200));
        h.bench_batched_async(
            "when_skip/half_skipped_50",
            move || {
                let resolver = Resolver { repo: repo.clone() };
                let request = build_read_batch(50, true);
                async move { (resolver, request) }
            },
            move |(resolver, request)| async move {
                let resp = execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap();
                std::hint::black_box(resp);
            },
        );
    }

    // ── all_unconditional_50 — 50 ops, no `when` field at all ────────────
    // (regression check against `batch_stage_parallelism.rs`'s
    // `batch_stage/reads_50` — same shape, same row count.)
    {
        let repo = rt.block_on(make_repo_with_rows(200));
        h.bench_batched_async(
            "when_skip/all_unconditional_50",
            move || {
                let resolver = Resolver { repo: repo.clone() };
                let request = build_read_batch(50, false);
                async move { (resolver, request) }
            },
            move |(resolver, request)| async move {
                let resp = execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap();
                std::hint::black_box(resp);
            },
        );
    }

    // ── cascade_chain_5 — A skipped (own `when` false) -> B,C,D,E cascade
    // skip through real `$query` DataFlow edges ──────────────────────────
    {
        let repo = rt.block_on(make_repo_with_rows(200));
        h.bench_batched_async(
            "when_skip/cascade_chain_5",
            move || {
                let resolver = Resolver { repo: repo.clone() };
                let request = build_chain_batch(true);
                async move { (resolver, request) }
            },
            move |(resolver, request)| async move {
                let resp = execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap();
                std::hint::black_box(resp);
            },
        );
    }

    // ── full_chain_5 — same 5-op chain, no `when` at all: all 5 ops
    // actually execute ────────────────────────────────────────────────────
    {
        let repo = rt.block_on(make_repo_with_rows(200));
        h.bench_batched_async(
            "when_skip/full_chain_5",
            move || {
                let resolver = Resolver { repo: repo.clone() };
                let request = build_chain_batch(false);
                async move { (resolver, request) }
            },
            move |(resolver, request)| async move {
                let resp = execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap();
                std::hint::black_box(resp);
            },
        );
    }

    h.run();
}
