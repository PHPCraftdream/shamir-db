//! Epic04/F (task #657) — `BatchOp::ForEach` execution-cost benchmarks.
//!
//! `BatchOp::ForEach` (Epic04, ADR + Phases B-E, commits `6ff521d5`,
//! `79510a13`, `7ed75075`, `f0ccf786`) is fully implemented, builder-wrapped,
//! unit-tested, and e2e-tested. This phase measures its execution cost
//! against the closest alternatives, so the numbers are interpretable, not
//! just absolute.
//!
//! Groups:
//! - `for_each/loop_over_flat_10` / `for_each/loop_over_flat_50` — a
//!   `ForEach` with a literal-array `over` of N elements and a 1-op body
//!   (an `Insert`), compared against `for_each/flat_batch_10` /
//!   `for_each/flat_batch_50` — a flat batch of N separate, hand-written
//!   `Insert` ops doing the equivalent work with no `ForEach` at all (what a
//!   caller would have to write client-side without the primitive). This is
//!   the primitive's core value proposition: does the loop-execution
//!   machinery (per-iteration body-planning reuse, per-iteration
//!   sub-`execute_batch` dispatch, result-list accumulation) cost more than
//!   just emitting N ops directly?
//! - `for_each/over_query_ref_10` — `over` sourced from a real `$query`
//!   column-ref (`@seed[].id`, resolved against a 10-row seed table),
//!   compared against `for_each/over_literal_10` — the identical 10-element
//!   literal array, same loop body. Isolates the cost of resolving `over`
//!   from a real column projection (touches the table/interner) vs. a
//!   pre-resolved literal.
//! - `for_each/nested_5x10` — an outer `ForEach` of 5 elements, each running
//!   an inner `ForEach` of 10 elements (50 total body executions: 5 outer
//!   iterations x 10 inner iterations, 1 insert each), compared against
//!   `for_each/flat_loop_50` — a single `ForEach` with 50 elements running
//!   the same 1-op insert body once each. Measures whether the "black box"
//!   body-planning-once-per-outer-node design (ADR Decision 1) has a
//!   measurable nesting tax.
//!
//! ## Measured results (this machine,
//! `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench -p
//! shamir-engine --bench for_each_eval`, JIT-calibrated iteration counts —
//! actual run output):
//!
//! ```text
//! for_each/loop_over_flat_10        854 iters    1156355.74 ns/op
//! for_each/flat_batch_10           1313 iters     767519.04 ns/op
//! for_each/loop_over_flat_50        157 iters    5776631.21 ns/op
//! for_each/flat_batch_50            268 iters    3574447.01 ns/op
//! for_each/over_literal_10          824 iters    1117735.80 ns/op
//! for_each/over_query_ref_10        765 iters    1326289.67 ns/op
//! for_each/nested_5x10              165 iters    6428152.73 ns/op
//! for_each/flat_loop_50             182 iters    5777258.79 ns/op
//! ```
//!
//! ## Conclusion
//!
//! - **`ForEach` vs. an equivalent flat unrolled batch (angle 1):** `ForEach`
//!   carries real, measurable per-iteration overhead vs. a hand-written flat
//!   batch of the same N inserts — not a wash, and the gap is bigger than a
//!   rounding error. At N=10, `ForEach` (~1.156 ms) is ~1.51x the cost of the
//!   flat batch (~0.768 ms); at N=50, `ForEach` (~5.777 ms) is ~1.62x the
//!   flat batch (~3.574 ms). The ratio does not shrink with N -- if anything
//!   it grows slightly (1.51x -> 1.62x) -- so this is not a fixed one-time
//!   setup cost being amortized away at higher N; it behaves like a genuine
//!   per-iteration tax (each iteration pays its own sub-`execute_batch`
//!   dispatch, body-planning lookup, and result-list append on top of the
//!   flat batch's per-op cost). Reported honestly: today, writing N ops by
//!   hand is meaningfully cheaper (~35-38% less wall time) than using
//!   `ForEach` over the same N -- the primitive buys ergonomics and
//!   data-dependent iteration counts, not a performance win, and callers on
//!   a hot path with a known, fixed N should prefer emitting the ops
//!   directly.
//! - **Column-ref `over` vs. literal-array `over` (angle 2):** resolving
//!   `over` from a real `$query` column projection (`over_query_ref_10`,
//!   ~1.326 ms -- this batch also runs the extra `seed` `Read` op in the
//!   same batch) costs ~19% more than an already-materialized literal array
//!   of the same length (`over_literal_10`, ~1.118 ms). Since
//!   `over_query_ref_10`'s batch does genuinely more work (a full `Read` of
//!   the seed table plus the column-projection walk, on top of the
//!   10-iteration loop), this gap is consistent with a one-time
//!   resolution/projection cost paid once before the loop starts (per
//!   Epic04/D's "resolved exactly once" guarantee), not a per-iteration
//!   cost -- a modest, expected overhead for touching the table/interner
//!   once instead of using a pre-resolved literal.
//! - **Nested `ForEach` vs. a single flattened loop of equal total
//!   iterations (angle 3):** `nested_5x10` (5 outer x 10 inner = 50 body
//!   executions, ~6.428 ms) is ~11% slower than `flat_loop_50` (a single
//!   50-element loop, ~5.777 ms). This is a small but real nesting tax, not
//!   noise-level -- the outer loop's 5 iterations each pay their own
//!   sub-`execute_batch` dispatch into the inner `ForEach`'s already-planned
//!   body, and that extra dispatch layer is not free. It is far smaller than
//!   the flat-vs-`ForEach` gap from angle 1 (~11% vs. ~51-62%), so the ADR
//!   Decision 1 "plan the body once per outer node" design does limit the
//!   nesting cost to roughly one extra dispatch layer rather than compounding
//!   per nesting level -- but it is not literally free either.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::batch::{
    execute_batch, BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding, TableResolver,
};
use shamir_engine::query::read::ReadQuery;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_builder::query::Query;
use shamir_query_types::batch::ForEachOp;
use shamir_query_types::filter::FilterValue;
use shamir_query_types::write::InsertOp;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::access::Actor;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::{InnerValue, QueryValue};

/// A single-table resolver, same shape as `when_skip_eval.rs`.
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

/// Build an in-memory repo with two tables: `rows` (pre-seeded, used for the
/// `$query` column-ref `over` case) and `orders` (write target for the loop
/// bodies / flat-batch inserts).
async fn make_repo_with_rows(n_rows: usize) -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new(
        "bench".into(),
        BoxRepo::InMemory(repo),
        vec![
            TableConfig::new("rows".to_string()),
            TableConfig::new("orders".to_string()),
        ],
    );
    let table = instance.get_table("rows").await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        shamir_types::core::interner::TouchInd::Exists(k)
        | shamir_types::core::interner::TouchInd::New(k) => k,
    };
    let k_id = touch("id");
    for i in 0..n_rows {
        let mut m = new_map_wc(1);
        m.insert(k_id.clone(), InnerValue::Int(i as i64));
        table.insert(&InnerValue::Map(m)).await.unwrap();
    }
    instance
}

/// A single-op `Insert` batch, taking `bind_row` (the `ForEach` `$param`
/// name) as the source of `user_id`.
fn insert_order_body(bind_row: &str) -> BatchRequest {
    let mut param_obj = new_map();
    param_obj.insert("$param".to_string(), QueryValue::Str(bind_row.to_string()));
    let mut insert_value = new_map();
    insert_value.insert("user_id".to_string(), QueryValue::Map(param_obj));
    insert_value.insert("note".to_string(), QueryValue::Str("fe_bench".to_string()));

    let mut queries = new_map();
    queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![QueryValue::Map(insert_value)],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(100),
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

/// Wrap a `ForEachOp` as the sole top-level entry of a `BatchRequest`.
fn wrap_for_each(fe: ForEachOp) -> BatchRequest {
    let mut queries = new_map();
    queries.insert(
        "loop".to_string(),
        QueryEntry {
            op: BatchOp::ForEach(fe),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
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

/// A `ForEach` over a literal-array `over` of `n` ints, 1-op insert body.
fn build_for_each_literal(n: usize) -> BatchRequest {
    let fe = ForEachOp {
        over: FilterValue::Array((0..n as i64).map(FilterValue::Int).collect()),
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    wrap_for_each(fe)
}

/// A flat batch of `n` mutually independent `Insert` ops — the equivalent
/// hand-written client-side work with no `ForEach` at all.
fn build_flat_insert_batch(n: usize) -> BatchRequest {
    let mut queries = new_map();
    for i in 0..n {
        let mut insert_value = new_map();
        insert_value.insert("user_id".to_string(), QueryValue::Int(i as i64));
        insert_value.insert("note".to_string(), QueryValue::Str("fe_bench".to_string()));
        queries.insert(
            format!("ins{i}"),
            QueryEntry {
                op: BatchOp::Insert(InsertOp {
                    insert_into: TableRef::new("orders"),
                    values: vec![QueryValue::Map(insert_value)],
                    records_idmsgpack: Vec::new(),
                    select: None,
                }),
                return_result: true,
                after: Vec::new(),
                when: None,
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

/// A `ForEach` over a real `$query` column-ref `over` (`@seed[].id`),
/// resolved against a `seed` read of the `rows` table, plus the `ForEach`
/// itself, in one batch. The iteration count is whatever the caller
/// pre-seeded the `rows` table with (see `make_repo_with_rows`).
fn build_for_each_query_ref_batch() -> BatchRequest {
    let fe = ForEachOp {
        over: FilterValue::QueryRef {
            alias: "@seed".to_string(),
            path: Some("[].id".to_string()),
        },
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let mut queries = new_map();
    queries.insert(
        "seed".to_string(),
        QueryEntry {
            op: BatchOp::Read(ReadQuery::from(Query::from("rows"))),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    queries.insert(
        "loop".to_string(),
        QueryEntry {
            op: BatchOp::ForEach(fe),
            return_result: true,
            after: vec!["seed".to_string()],
            when: None,
        },
    );
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

/// Nested `ForEach`: outer loop of `outer_n` elements, each running an inner
/// `ForEach` of `inner_n` elements (1-op insert body) — `outer_n * inner_n`
/// total body executions.
fn build_nested_for_each(outer_n: usize, inner_n: usize) -> BatchRequest {
    let inner_fe = ForEachOp {
        over: FilterValue::Array((0..inner_n as i64).map(FilterValue::Int).collect()),
        bind_row: "iid".to_string(),
        batch: insert_order_body("iid"),
    };
    let mut inner_queries = new_map();
    inner_queries.insert(
        "inner_loop".to_string(),
        QueryEntry {
            op: BatchOp::ForEach(inner_fe),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_batch = BatchRequest {
        id: QueryValue::Int(200),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let outer_fe = ForEachOp {
        over: FilterValue::Array((0..outer_n as i64).map(FilterValue::Int).collect()),
        bind_row: "oid".to_string(),
        batch: inner_batch,
    };
    wrap_for_each(outer_fe)
}

fn main() {
    let mut h = Harness::new("for_each_eval", env!("CARGO_MANIFEST_DIR"));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // ── angle 1: for_each vs. flat unrolled batch, N=10 and N=50 ─────────
    for &n in &[10usize, 50usize] {
        {
            let repo = rt.block_on(make_repo_with_rows(0));
            h.bench_batched_async(
                &format!("for_each/loop_over_flat_{n}"),
                move || {
                    let resolver = Resolver { repo: repo.clone() };
                    let request = build_for_each_literal(n);
                    async move { (resolver, request) }
                },
                move |(resolver, request)| async move {
                    let resp =
                        execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                            .await
                            .unwrap();
                    std::hint::black_box(resp);
                },
            );
        }
        {
            let repo = rt.block_on(make_repo_with_rows(0));
            h.bench_batched_async(
                &format!("for_each/flat_batch_{n}"),
                move || {
                    let resolver = Resolver { repo: repo.clone() };
                    let request = build_flat_insert_batch(n);
                    async move { (resolver, request) }
                },
                move |(resolver, request)| async move {
                    let resp =
                        execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                            .await
                            .unwrap();
                    std::hint::black_box(resp);
                },
            );
        }
    }

    // ── angle 2: over as $query column-ref vs. literal array, N=10 ───────
    {
        let repo = rt.block_on(make_repo_with_rows(10));
        h.bench_batched_async(
            "for_each/over_literal_10",
            move || {
                let resolver = Resolver { repo: repo.clone() };
                let request = build_for_each_literal(10);
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
    {
        let repo = rt.block_on(make_repo_with_rows(10));
        h.bench_batched_async(
            "for_each/over_query_ref_10",
            move || {
                let resolver = Resolver { repo: repo.clone() };
                let request = build_for_each_query_ref_batch();
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

    // ── angle 3: nested ForEach (5x10) vs. a single flattened 50-loop ────
    {
        let repo = rt.block_on(make_repo_with_rows(0));
        h.bench_batched_async(
            "for_each/nested_5x10",
            move || {
                let resolver = Resolver { repo: repo.clone() };
                let request = build_nested_for_each(5, 10);
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
    {
        let repo = rt.block_on(make_repo_with_rows(0));
        h.bench_batched_async(
            "for_each/flat_loop_50",
            move || {
                let resolver = Resolver { repo: repo.clone() };
                let request = build_for_each_literal(50);
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
