//! RV-QUERY-BUILDER-E8D88CD7 regression: a HAVING operand that references a
//! known batch alias must produce a DataFlow dependency edge in the plan.
//!
//! Before the fix, `BatchPlanner::plan`'s `BatchOp::Read` arm extracted
//! references only from WHERE and SELECT items, so a read whose ONLY
//! reference to alias `a` lived in `GROUP BY ... HAVING` planned with no
//! dependency on `a`. The runner's `build_resolved_refs` then exposed no
//! `a` result into the read's `FilterContext`, and the HAVING comparison
//! against `a`'s result folded to Null — silently keeping or dropping the
//! wrong groups.

use shamir_collections::new_map;
use shamir_query_types::batch::{BatchLimits, BatchPlanner, EdgeKind, QueryEntry};
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::read::{GroupBy, ReadQuery};
use shamir_query_types::ResultEncoding;

#[test]
fn result_encoding_is_exported_at_crate_root() {
    assert_eq!(ResultEncoding::default(), ResultEncoding::Name);
}

/// A read whose only reference to producer alias `a` sits inside a HAVING
/// filter must plan a DataFlow edge `b -> a`, so the runner's
/// `build_resolved_refs` (which exposes results only for DataFlow/Both
/// edges) supplies `a`'s result to the HAVING evaluation.
///
/// Fails on the unfixed planner: no HAVING extraction meant `dependencies`
/// for `b` was empty, the plan built successfully (both aliases exist), and
/// this assertion on the dependency set / provenance failed.
#[test]
fn rv_query_builder_e8d88cd7_having_only_query_ref_plans_dataflow_edge() {
    let producer = ReadQuery::new("events");
    let consumer = ReadQuery::new("orders").group_by(GroupBy::new(["uid"]).having(Filter::Eq {
        field: vec!["total".to_string()],
        value: FilterValue::QueryRef {
            alias: "producer".to_string(),
            path: Some("[].sum".to_string()),
        },
    }));

    let mut queries = new_map();
    queries.insert("producer".to_string(), QueryEntry::from(producer));
    queries.insert("consumer".to_string(), QueryEntry::from(consumer));

    let plan = BatchPlanner::plan(&queries, &BatchLimits::default())
        .expect("HAVING-only `$query` reference to a known alias must plan");

    let deps = plan
        .dependencies
        .get("consumer")
        .expect("consumer entry present in plan");
    assert!(
        deps.contains("producer"),
        "HAVING-only reference must create a dependency on `producer`, got {:?}",
        deps
    );

    let provenance = plan
        .edge_provenance
        .get("consumer")
        .and_then(|p| p.get("producer"))
        .copied()
        .expect("producer edge must carry provenance");
    assert_eq!(
        provenance,
        EdgeKind::DataFlow,
        "edge must be DataFlow so `build_resolved_refs` exposes the producer's result"
    );
}
