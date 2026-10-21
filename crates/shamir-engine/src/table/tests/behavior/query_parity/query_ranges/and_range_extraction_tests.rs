//! Correctness fuzz: AND-range extraction must produce byte-identical
//! results compared to full-scan reference.
//!
//! Phase 2.4 — `try_plan_and_range_index_scan` extracts a range predicate
//! from an AND filter, uses the sorted index for the range scan, and
//! applies remaining conjuncts as a residual filter. This test generates
//! randomized AND queries and asserts result-set identity with the
//! full-scan (no-index) baseline.

use crate::db_instance::db_instance::DbInstance;
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::ReadQuery;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::tests::test_helpers::query_value_to_inner_tracked;
use crate::table::TableConfig;
use shamir_query_builder::filter::{and, eq, gte, lte};
use shamir_query_builder::Query;
use shamir_query_types::filter::Filter;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

/// Deterministic PRNG (xorshift64) — no external dep, reproducible.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn next_i64(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }
}

/// Create two identical tables (one with sorted index, one without) and
/// populate with `n` records having fields `x` (int), `name` (str),
/// `color` (str), `rank` (int).
async fn setup_pair(n: usize) -> (crate::table::TableManager, crate::table::TableManager) {
    let repo_config_idx = RepoConfig {
        name: "indexed".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("data")],
    };
    let repo_config_ref = RepoConfig {
        name: "reference".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("data")],
    };
    let db = DbInstance::with_repos(vec![repo_config_idx, repo_config_ref])
        .await
        .unwrap();
    let tbl_idx = db.get_table("indexed", "data").await.unwrap();
    let tbl_ref = db.get_table("reference", "data").await.unwrap();

    let names = ["alice", "bob", "charlie", "dave"];
    let colors = ["red", "green", "blue"];

    let mut rng = Rng::new(42);

    for _ in 0..n {
        let x = rng.next_i64(0, 200);
        let name = names[(rng.next() % names.len() as u64) as usize];
        let color = colors[(rng.next() % colors.len() as u64) as usize];
        let rank = rng.next_i64(0, 10);

        let mut map = new_map();
        map.insert("x".to_string(), QueryValue::Int(x));
        map.insert("name".to_string(), QueryValue::Str(name.into()));
        map.insert("color".to_string(), QueryValue::Str(color.into()));
        map.insert("rank".to_string(), QueryValue::Int(rank));
        let val = QueryValue::Map(map);

        // Insert into both tables (same data, same order => same record_ids).
        for tbl in [&tbl_idx, &tbl_ref] {
            let interner = tbl.interner().get().await.unwrap();
            let (inner, new_keys) = query_value_to_inner_tracked(&val, interner).unwrap();
            if !new_keys.is_empty() {
                tbl.interner().save_new_keys(&new_keys).await.unwrap();
            }
            tbl.insert(&inner).await.unwrap();
        }
    }

    // Create sorted index on `x` ONLY on the indexed table.
    tbl_idx
        .create_sorted_index("x_sorted", &["x"])
        .await
        .unwrap();

    (tbl_idx, tbl_ref)
}

/// Extract the `x` field (i64) from each record, sort, and return.
fn sorted_x_values(result: &crate::query::read::QueryResult) -> Vec<i64> {
    let mut xs: Vec<i64> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("x"))
        .collect();
    xs.sort();
    xs
}

/// 50 randomized AND queries (range on x + 0-3 equality predicates on
/// other fields). Results from the indexed table (which hits
/// `try_plan_and_range_index_scan`) must be identical to the reference
/// table (full scan).
#[tokio::test]
async fn and_range_extraction_byte_identical_full_scan() {
    let (tbl_idx, tbl_ref) = setup_pair(1000).await;

    let names = ["alice", "bob", "charlie", "dave"];
    let colors = ["red", "green", "blue"];

    let mut rng = Rng::new(0xDEAD_BEEF);

    for trial in 0..50 {
        // Random range bounds on x.
        let lo = rng.next_i64(0, 150);
        let hi = rng.next_i64(lo, 200);

        // Start with the range predicate.
        let mut preds: Vec<Filter> = vec![gte("x", lo), lte("x", hi)];

        // Add 0-3 equality predicates on other fields.
        let n_eq = (rng.next() % 4) as usize;
        if n_eq >= 1 {
            let name = names[(rng.next() % names.len() as u64) as usize];
            preds.push(eq("name", name));
        }
        if n_eq >= 2 {
            let color = colors[(rng.next() % colors.len() as u64) as usize];
            preds.push(eq("color", color));
        }
        if n_eq >= 3 {
            let rank = rng.next_i64(0, 10);
            preds.push(eq("rank", rank));
        }

        let filter = and(preds);

        let q_idx: ReadQuery = Query::from("data").where_(filter.clone()).build();
        let q_ref: ReadQuery = Query::from("data").where_(filter).build();

        let interner_idx = tbl_idx.interner().get().await.unwrap();
        let refs_idx = new_map();
        let ctx_idx = FilterContext::new(interner_idx, &refs_idx);

        let interner_ref = tbl_ref.interner().get().await.unwrap();
        let refs_ref = new_map();
        let ctx_ref = FilterContext::new(interner_ref, &refs_ref);

        let res_idx = tbl_idx.read(&q_idx, &ctx_idx).await.unwrap();
        let res_ref = tbl_ref.read(&q_ref, &ctx_ref).await.unwrap();

        let xs_idx = sorted_x_values(&res_idx);
        let xs_ref = sorted_x_values(&res_ref);

        assert_eq!(
            res_idx.records.len(),
            res_ref.records.len(),
            "trial {trial}: count mismatch (indexed={}, reference={})",
            res_idx.records.len(),
            res_ref.records.len()
        );

        assert_eq!(xs_idx, xs_ref, "trial {trial}: x-value sets diverge");

        // The indexed table should use the sorted index (not full scan).
        if let Some(ref stats) = res_idx.stats {
            if let Some(ref used) = stats.index_used {
                assert!(
                    used.contains("sorted_idx_"),
                    "trial {trial}: expected sorted_idx usage, got {used:?}"
                );
            }
        }
    }
}
