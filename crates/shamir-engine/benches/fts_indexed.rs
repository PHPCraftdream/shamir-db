//! Full-text search benchmark: indexed FTS vs brute-force FTS,
//! both through the engine's `TableManager::read` path.
//!
//! Story: `filter_eval.rs` already benches `Filter::matches()` directly
//! for `Fts { mode: "and" }` / `Fts { mode: "or" }` over 1000 records —
//! that's the *predicate cost only*, no record materialisation. The
//! repository ships a real FTS index (`shamir-index` + `index2`
//! registry) wired into `TableManager::read` via
//! `try_plan_index2`, but the win of "don't scan 1000 docs to answer
//! an FTS query" has never been pinned to a number.
//!
//! This file runs the **same** FTS query through the same end-to-end
//! `table.read(&q, &ctx)` path twice:
//!
//!   - `indexed_*` — the table has an FTS index on `body`.
//!     `try_plan_index2` matches, and we get a record-id set from the
//!     postings index. `read_exec.rs` then fetches just those rows.
//!   - `brute_*`   — the table has **no** FTS index. The same query
//!     falls through to the brute scan path (`Filter::matches()` per
//!     record), exactly like `fts_brute_*_1000` in `filter_eval.rs`
//!     but now including the `get_many` / materialisation tail
//!     so both numbers measure the *same units of work*.
//!
//! Corpus mirrors `filter_eval.rs`: 1000 records whose searched field
//! contains `format!("user-{idx}")`. Query is `"user alpha"` —
//! token "user" matches every doc, "alpha" matches none, so:
//!   - mode "and" → 0 hits  (indexed: postings intersection is empty)
//!   - mode "or"  → 1000 hits (indexed: full "user" posting list returned)
//!
//! Phrase mode is intentionally **not** benched: the engine's FTS
//! backend exposes only `FtsMode::AndAll` / `FtsMode::OrAny`
//! (`crates/shamir-engine/.../index2/backend.rs`) — there is no
//! phrase-with-positions path to measure today.
//!
//! Long-corpus variant (N=100_000) is gated behind the
//! `fts_indexed_long` cargo feature, since seeding the fixture takes
//! several seconds. Run with:
//!
//! ```text
//! cargo bench --bench fts_indexed -p shamir-engine --features fts_indexed_long
//! ```
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): each
//! `TableManager` fixture (indexed / brute, per N) is built ONCE outside
//! the timed closure — plan 1 (shared setup) — reads are driven via
//! `bench_async` against the harness-owned shared runtime.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::eval_context::FilterContext;
use shamir_engine::query::read::ReadQuery;
use shamir_engine::table::table_manager::TableManager;
use shamir_query_builder::filter::fts;
use shamir_query_builder::query::Query;
use shamir_query_types::batch::BatchOp;

// DDL: typed builder → BatchOp::CreateIndex(op) → engine's
// `create_index_v2`. We pattern-match the BatchOp to extract the
// `CreateIndexOp` payload (per CLAUDE.md, all DDL goes through the
// builder; the unwrap is a bench-only assertion that the builder
// produced the variant we asked for).
use shamir_query_builder::ddl;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::InnerValue;

/// Corpus sizes benched. N=1000 mirrors the predicate-only baseline in
/// `filter_eval.rs`; N=10_000 widens the asymptotic gap between the
/// indexed and brute paths (the per-call materialisation tail no longer
/// dominates at 10k rows) — a genuine algorithmic-scaling comparison, not
/// a cheap-call-repeated-N-times artifact, so it stays as a sweep rather
/// than collapsing to one size. Default sweep is N=1000 only (each
/// brute-force call at N=10_000 costs ~6-7ms, too close to the harness's
/// ~10ms/call budget for the FAST default); N=10_000 is opt-in via
/// BENCH_FTS_INDEXED_SCALING=1.
fn corpus_sizes() -> Vec<usize> {
    let mut ns = vec![1000];
    let wide = std::env::var("BENCH_FTS_INDEXED_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if wide {
        ns.push(10_000);
    }
    ns
}

/// Build a `docs` TableManager populated with `n` records whose
/// `body` field holds `format!("user-{i}")`. When `with_fts_index` is
/// true, an FTS (whitespace tokenizer) index on `body` is created
/// **before** inserts so postings are populated incrementally.
async fn build_docs_table(n: usize, with_fts_index: bool) -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = TableManager::create("docs".into(), data, info)
        .await
        .unwrap();

    if with_fts_index {
        let op = ddl::create_index("body_fts", "docs")
            .field("body")
            .index_type("fts")
            .fts_tokenizer("whitespace")
            .build();
        let create_op = match op {
            BatchOp::CreateIndex(o) => o,
            other => panic!("expected BatchOp::CreateIndex from builder, got {other:?}"),
        };
        mgr.create_index_v2(&create_op).await.unwrap();
    }

    let body_k = {
        let i = mgr.interner().get().await.unwrap();
        match i.touch_ind("body").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k,
        }
    };

    for i in 0..n {
        let mut m = new_map_wc(1);
        m.insert(body_k.clone(), InnerValue::Str(format!("user-{i}")));
        mgr.insert(&InnerValue::Map(m)).await.unwrap();
    }

    mgr
}

fn build_query(mode: &str) -> ReadQuery {
    Query::from("docs")
        .where_(fts("body", "user alpha", mode))
        .build()
}

/// N=100_000 long variant fixture. Opt-in via `--features fts_indexed_long`.
/// Mirrors the selective-query shape (rare token in 1% of docs) so the
/// indexed/brute ratio reflects the asymptotic FTS-index win at scale.
const SELECTIVE_N: usize = 1000;
const SELECTIVE_HIT_RATIO: usize = 100; // every 100th doc has the rare token

async fn build_docs_table_selective(n: usize, with_fts_index: bool) -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = TableManager::create("docs".into(), data, info)
        .await
        .unwrap();

    if with_fts_index {
        let op = ddl::create_index("body_fts", "docs")
            .field("body")
            .index_type("fts")
            .fts_tokenizer("whitespace")
            .build();
        let create_op = match op {
            BatchOp::CreateIndex(o) => o,
            other => panic!("expected BatchOp::CreateIndex from builder, got {other:?}"),
        };
        mgr.create_index_v2(&create_op).await.unwrap();
    }

    let body_k = {
        let i = mgr.interner().get().await.unwrap();
        match i.touch_ind("body").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k,
        }
    };

    for i in 0..n {
        let mut m = new_map_wc(1);
        let body = if i % SELECTIVE_HIT_RATIO == 0 {
            // ~1% of docs carry the rare token.
            format!("user-{i} needle")
        } else {
            format!("user-{i}")
        };
        m.insert(body_k.clone(), InnerValue::Str(body));
        mgr.insert(&InnerValue::Map(m)).await.unwrap();
    }

    mgr
}

fn build_selective_query() -> ReadQuery {
    // Single rare token; mode irrelevant for a single-token query but
    // pick "or" for parity with the existing group.
    Query::from("docs")
        .where_(fts("body", "needle", "or"))
        .build()
}

fn register_read(h: &mut Harness, id: &str, table: TableManager, q: ReadQuery) {
    h.bench_async(id, move || {
        let table = table.clone();
        let q = q.clone();
        async move {
            let interner = table.interner().get().await.unwrap();
            let refs = new_map();
            let ctx = FilterContext::new(interner, &refs);
            let r = table.read(&q, &ctx).await.unwrap();
            std::hint::black_box(r);
        }
    });
}

fn main() {
    let mut h = Harness::new("fts_indexed", env!("CARGO_MANIFEST_DIR"));

    let setup_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let q_and = build_query("and");
    let q_or = build_query("or");

    for n in corpus_sizes() {
        let indexed_table = setup_rt.block_on(build_docs_table(n, true));
        let brute_table = setup_rt.block_on(build_docs_table(n, false));

        register_read(
            &mut h,
            &format!("fts_indexed_vs_brute/indexed_and_{n}"),
            indexed_table.clone(),
            q_and.clone(),
        );
        register_read(
            &mut h,
            &format!("fts_indexed_vs_brute/indexed_or_{n}"),
            indexed_table,
            q_or.clone(),
        );
        register_read(
            &mut h,
            &format!("fts_indexed_vs_brute/brute_and_via_read_{n}"),
            brute_table.clone(),
            q_and.clone(),
        );
        register_read(
            &mut h,
            &format!("fts_indexed_vs_brute/brute_or_via_read_{n}"),
            brute_table,
            q_or.clone(),
        );
    }

    // --- Selective-query group ------------------------------------------------
    //
    // The `fts_indexed_vs_brute` group above queries "user alpha" — token
    // "user" matches every doc, so the indexed "or" path returns the full
    // corpus and both paths pay the same materialisation tail. To expose
    // the index's real win we need a SELECTIVE query: a rare token that
    // hits ~1% of documents. The indexed path then fetches ~N/100 RIDs
    // directly from the postings; the brute path still scans every row.
    {
        let q = build_selective_query();
        let n = SELECTIVE_N;
        let indexed_table = setup_rt.block_on(build_docs_table_selective(n, true));
        let brute_table = setup_rt.block_on(build_docs_table_selective(n, false));

        register_read(
            &mut h,
            &format!("fts_indexed_selective/indexed_selective_{n}"),
            indexed_table,
            q.clone(),
        );
        register_read(
            &mut h,
            &format!("fts_indexed_selective/brute_selective_via_read_{n}"),
            brute_table,
            q,
        );
    }

    // --- N=100_000 long variant (feature-gated) -------------------------------
    //
    // Opt-in via `--features fts_indexed_long`. Mirrors the selective-query
    // shape (rare token in 1% of docs) so the indexed/brute ratio reflects
    // the asymptotic FTS-index win at scale.
    #[cfg(feature = "fts_indexed_long")]
    {
        let q = build_selective_query();
        let n: usize = 100_000;
        let indexed_table = setup_rt.block_on(build_docs_table_selective(n, true));
        let brute_table = setup_rt.block_on(build_docs_table_selective(n, false));

        register_read(
            &mut h,
            &format!("fts_indexed_long/indexed_selective_{n}"),
            indexed_table,
            q.clone(),
        );
        register_read(
            &mut h,
            &format!("fts_indexed_long/brute_selective_via_read_{n}"),
            brute_table,
            q,
        );
    }

    h.run();
}
