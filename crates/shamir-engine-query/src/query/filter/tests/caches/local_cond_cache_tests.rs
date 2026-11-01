//! Tests for `FilterContext::local_cond_cache` — the lazily-populated
//! fallback `$cond` cache (group 13 Defect 1).
//!
//! Before this fix, `resolve_filter_query`'s `FilterValue::Cond` arm
//! recompiled `cond.condition` (e.g. re-running `Regex::new`) on EVERY
//! evaluation whenever `ctx.cond_cache` was `None` — which is every WHERE,
//! `when`, `for_each`, and write-value evaluation path, since none of them
//! pre-scan an explicit `CondCache` the way `SelectProjection::new` does.
//! For a `$cond` whose condition contains a `Regex`/`Like` pattern, that
//! meant recompiling the regex once per row scanned.
//!
//! These tests prove the fix directly and deterministically — no
//! wall-clock timing, which would be flaky under load. `ctx.local_cond_cache`
//! is `pub(crate)`, so a test can inspect its size/contents directly:
//! exactly ONE compiled entry after N evaluations of the SAME condition
//! proves `compile_filter` ran once, not N times.

use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::cond_cache::{compile_cond_cached, new_local_cond_cache};
use crate::query::filter::eval::resolve_filter_query;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Cond, Filter, FilterValue};
use crate::query::read::QueryResult;

fn empty_refs() -> TMap<String, QueryResult> {
    new_map()
}

fn make_score_record(interner: &Interner, score: i64) -> InnerValue {
    let mut map = new_map();
    let k_score = interner.touch_ind("score").unwrap().into_key();
    map.insert(k_score, InnerValue::Int(score));
    InnerValue::Map(map)
}

/// A `$cond` testing `score > 50 ? "high" : "low"` — mirrors
/// `cond_cache_tests.rs`'s fixture (WHERE-clause shape: threshold
/// comparison, no cache pre-scan).
fn make_score_cond() -> Cond {
    Cond::new(
        Filter::Gt {
            field: vec!["score".to_string()],
            value: FilterValue::Int(50),
        },
        FilterValue::String("high".to_string()),
        FilterValue::String("low".to_string()),
    )
}

/// A `$cond` whose condition is a `Regex` match — the review's concrete
/// motivating hazard: `Regex::new` is expensive to run per row.
fn make_regex_cond() -> Cond {
    Cond::new(
        Filter::Regex {
            field: vec!["name".to_string()],
            pattern: "^a.*e$".to_string(),
        },
        FilterValue::String("matched".to_string()),
        FilterValue::String("unmatched".to_string()),
    )
}

fn make_name_record(interner: &Interner, name: &str) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().into_key();
    map.insert(k_name, InnerValue::Str(name.to_string()));
    InnerValue::Map(map)
}

/// THE decisive test: evaluating the SAME `$cond` (via the SAME
/// `FilterContext`, never opted into the eager `cond_cache` — the
/// WHERE/`when`/write-value shape) against MANY records must compile
/// `cond.condition` EXACTLY ONCE, not once per row.
#[test]
fn where_clause_reuses_compiled_cond_across_many_rows() {
    let interner = Interner::new();
    interner.touch_ind("score").unwrap();

    let cond = make_score_cond();
    let fv = FilterValue::Cond {
        cond: Box::new(cond),
    };

    let refs = empty_refs();
    // No `.with_cond_cache(..)` — `ctx.cond_cache` stays `None`, so the
    // `Cond` arm must fall back to `ctx.local_cond_cache`.
    let ctx = FilterContext::new(&interner, &refs);

    // Evaluate against 200 distinct records straddling the threshold.
    for i in 0..200i64 {
        let record = make_score_record(&interner, i);
        let expected = if i > 50 { "high" } else { "low" };
        assert_eq!(
            resolve_filter_query(&fv, &record, &ctx),
            Some(QueryValue::Str(expected.to_string())),
            "row {i} must resolve correctly through the fallback cache path"
        );
    }

    // The decisive assertion: exactly ONE compiled entry after 200
    // evaluations of the SAME condition — proving `compile_filter` ran
    // once, not 200 times. `len()` is O(N) on `scc::HashMap` (banned on
    // hot paths, see clippy.toml) but this is a test-only assertion over a
    // handful of entries, not a production code path.
    #[allow(clippy::disallowed_methods)] // O(N) ack: test-only assertion, tiny N
    let entries = ctx.local_cond_cache.len();
    assert_eq!(
        entries, 1,
        "local_cond_cache must hold exactly one compiled entry after 200 \
         evaluations of the SAME $cond — proving the condition was \
         compiled ONCE and reused, not recompiled per row"
    );
}

/// The same reuse proof for the review's concrete motivating case: a
/// `$cond` whose condition is a `Regex` pattern. `Regex::new` (inside
/// `compile_filter`) must run exactly once across many rows.
#[test]
fn where_clause_reuses_compiled_regex_cond_across_many_rows() {
    let interner = Interner::new();
    interner.touch_ind("name").unwrap();

    let cond = make_regex_cond();
    let fv = FilterValue::Cond {
        cond: Box::new(cond),
    };

    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let names = ["alice", "andre", "bob", "anne", "carol"];
    for name in names {
        let record = make_name_record(&interner, name);
        let matches_pattern = name.starts_with('a') && name.ends_with('e');
        let expected = if matches_pattern {
            "matched"
        } else {
            "unmatched"
        };
        assert_eq!(
            resolve_filter_query(&fv, &record, &ctx),
            Some(QueryValue::Str(expected.to_string())),
            "name={name} must resolve correctly through the fallback cache path"
        );
    }

    #[allow(clippy::disallowed_methods)] // O(N) ack: test-only assertion, tiny N
    let entries = ctx.local_cond_cache.len();
    assert_eq!(
        entries, 1,
        "local_cond_cache must hold exactly one compiled entry (the Regex \
         compiled once, not once per row)"
    );
}

/// Two DIFFERENT `$cond` conditions evaluated against the SAME context
/// (e.g. two `$cond`s inside one WHERE clause) must occupy two SEPARATE
/// cache entries — the fallback cache must not merge or confuse distinct
/// conditions sharing one `FilterContext`.
#[test]
fn distinct_conditions_get_distinct_cache_entries() {
    let interner = Interner::new();
    interner.touch_ind("score").unwrap();
    interner.touch_ind("name").unwrap();

    let score_fv = FilterValue::Cond {
        cond: Box::new(make_score_cond()),
    };
    let regex_fv = FilterValue::Cond {
        cond: Box::new(make_regex_cond()),
    };

    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let mut record = new_map();
    record.insert(
        interner.touch_ind("score").unwrap().into_key(),
        InnerValue::Int(80),
    );
    record.insert(
        interner.touch_ind("name").unwrap().into_key(),
        InnerValue::Str("andre".to_string()),
    );
    let record = InnerValue::Map(record);

    assert_eq!(
        resolve_filter_query(&score_fv, &record, &ctx),
        Some(QueryValue::Str("high".to_string()))
    );
    assert_eq!(
        resolve_filter_query(&regex_fv, &record, &ctx),
        Some(QueryValue::Str("matched".to_string()))
    );

    #[allow(clippy::disallowed_methods)] // O(N) ack: test-only assertion, tiny N
    let entries = ctx.local_cond_cache.len();
    assert_eq!(
        entries, 2,
        "two structurally different $cond conditions sharing one context \
         must occupy two distinct cache entries"
    );
}

/// Direct unit proof of `compile_cond_cached`'s reuse contract: calling it
/// twice with the SAME condition and cache returns the SAME `Arc<FilterNode>`
/// allocation (`Arc::ptr_eq`), not two independently-compiled nodes that
/// merely produce equal results. This is the strongest possible evidence
/// against a "recompiles every time but happens to agree" false negative.
#[test]
fn compile_cond_cached_returns_the_same_arc_on_repeated_calls() {
    let interner = Interner::new();
    interner.touch_ind("score").unwrap();

    let cond = make_score_cond();
    let cache = new_local_cond_cache();

    let first = compile_cond_cached(&cache, &cond.condition, &interner).unwrap();
    let second = compile_cond_cached(&cache, &cond.condition, &interner).unwrap();
    let third = compile_cond_cached(&cache, &cond.condition, &interner).unwrap();

    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "compile_cond_cached must return the SAME Arc allocation on the \
         second call for an unchanged condition — proving genuine reuse, \
         not a coincidentally-equal recompile"
    );
    assert!(
        std::sync::Arc::ptr_eq(&second, &third),
        "compile_cond_cached must return the SAME Arc allocation on the \
         third call too"
    );
}

// L3-RV-ENGINE-62C6F2B1 — the cache is now ADDRESS-keyed (usize) with a
// structural-hash pre-filter plus full `PartialEq` content re-check on hits;
// no `format!("{:?}")` on any path. The tests below pin the guard: a stale
// entry left at a reused address (freed condition, later reallocated to a
// different one) must NEVER be served, and each distinct condition must
// resolve to its own compiled node.

/// A distinct filter used for collision/reuse fixtures.
fn make_age_cond() -> Box<Cond> {
    Box::new(Cond::new(
        Filter::Lt {
            field: vec!["age".to_string()],
            value: FilterValue::Int(18),
        },
        FilterValue::String("minor".to_string()),
        FilterValue::String("adult".to_string()),
    ))
}

/// Deterministic stale-address proof: a bogus entry is injected directly at
/// condition B's address key with a DIFFERENT filter's content (the exact
/// state a dropped condition + allocator reuse produces). `compile_cond_cached`
/// must detect the content mismatch, evict the stale entry, and return a
/// freshly compiled node for B — never the stale one. (This is deterministic
/// where real allocator address reuse is not.)
#[test]
fn compile_cond_cached_rejects_stale_entry_at_reused_address() {
    let interner = Interner::new();
    interner.touch_ind("score").unwrap();
    interner.touch_ind("age").unwrap();

    let cond_b = make_age_cond();
    let cache = new_local_cond_cache();

    // Inject the stale entry exactly as the old bug would leave it: keyed by
    // B's address, but holding a DIFFERENT filter (score > 50) and node.
    let stale_cond = make_score_cond();
    let stale_node = compile_cond_cached(&cache, &stale_cond.condition, &interner).unwrap();
    let addr_b = &*cond_b.condition as *const Filter as usize;
    let stale_addr = &*stale_cond.condition as *const Filter as usize;
    assert_ne!(addr_b, stale_addr);
    let stale_entry = (
        crate::query::filter::cond_cache::structural_hash_for_tests(&stale_cond.condition),
        (*stale_cond.condition).clone(),
        std::sync::Arc::clone(&stale_node),
    );
    if cache.insert_sync(addr_b, stale_entry).is_err() {
        panic!("no concurrent writer in the test");
    }

    let fresh = compile_cond_cached(&cache, &cond_b.condition, &interner).expect("compile ok");
    assert!(
        !std::sync::Arc::ptr_eq(&fresh, &stale_node),
        "a different condition parked at a reused address must NEVER be \
         served the stale entry's compiled node"
    );
    // The stale entry was evicted and replaced by B's own entry; a repeat
    // call now hits B's genuine entry (same Arc again).
    let again = compile_cond_cached(&cache, &cond_b.condition, &interner).expect("compile ok");
    assert!(std::sync::Arc::ptr_eq(&fresh, &again));
}

/// Two distinct conditions at distinct addresses: each gets and keeps its
/// OWN compiled node — no cross-serve even though both live in one cache.
#[test]
fn compile_cond_cached_distinct_conditions_get_distinct_nodes() {
    let interner = Interner::new();
    interner.touch_ind("score").unwrap();
    interner.touch_ind("age").unwrap();

    let cond_a = make_score_cond();
    let cond_b = make_age_cond();
    let cache = new_local_cond_cache();

    let node_a = compile_cond_cached(&cache, &cond_a.condition, &interner).unwrap();
    let node_b = compile_cond_cached(&cache, &cond_b.condition, &interner).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&node_a, &node_b));
    // Repeated lookups stay on their own entries.
    let node_a2 = compile_cond_cached(&cache, &cond_a.condition, &interner).unwrap();
    let node_b2 = compile_cond_cached(&cache, &cond_b.condition, &interner).unwrap();
    assert!(std::sync::Arc::ptr_eq(&node_a, &node_a2));
    assert!(std::sync::Arc::ptr_eq(&node_b, &node_b2));
}
