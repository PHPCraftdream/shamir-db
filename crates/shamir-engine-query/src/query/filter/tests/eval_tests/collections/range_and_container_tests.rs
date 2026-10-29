//! Collection-membership filter tests: In/NotIn over literals, QueryRef
//! columns, InSet and $contains variants.

use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_types::core::interner::Interner;

use crate::query::filter::tests::eval_tests::shared::helpers::{
    empty_refs, make_alice_record, make_date_record, make_list_record, make_set_record,
};
// ============================================================================
// Between
// ============================================================================

#[test]
fn test_between_in_range() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["age".to_string()],
        from: FilterValue::Int(25),
        to: FilterValue::Int(35),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 30 is between 25 and 35
}

#[test]
fn test_between_at_lower_bound() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["age".to_string()],
        from: FilterValue::Int(30),
        to: FilterValue::Int(40),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 30 >= 30
}

#[test]
fn test_between_at_upper_bound() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["age".to_string()],
        from: FilterValue::Int(20),
        to: FilterValue::Int(30),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 30 <= 30
}

#[test]
fn test_between_out_of_range() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["age".to_string()],
        from: FilterValue::Int(31),
        to: FilterValue::Int(40),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // 30 < 31
}

#[test]
fn test_between_with_field_ref() {
    let interner = Interner::new();
    let record = make_date_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // start_date (100) between 50 and end_date (200)
    let filter = Filter::Between {
        field: vec!["start_date".to_string()],
        from: FilterValue::Int(50),
        to: FilterValue::FieldRef {
            path: vec!["end_date".to_string()],
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 100 between 50 and 200
}

#[test]
fn test_between_string() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["name".to_string()],
        from: FilterValue::String("A".to_string()),
        to: FilterValue::String("B".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "Alice" between "A" and "B"
}

// ============================================================================
// F6 regression: $in / $nin against a NON-scalar (container) field.
//
// `FilterNode::InSet` (the all-literal fast path for `$in`/`$nin`) now uses
// `record.scalar_at` + `set_contains_coercing` — the SAME borrow-based probe
// the sibling `FilterNode::In` (the dynamic / mixed-literal path) uses. A
// non-scalar field (List / Set / Map / Bin) returns `None` from `scalar_at`,
// so both nodes treat it as ABSENT: `$in` → false, `$nin` → true. These
// tests pin that the two sibling nodes agree on this edge case (the
// inconsistency F6 fixed was that `InSet` previously walked INTO the
// container via `materialize_at`, while `In` already treated it as absent).
// ============================================================================

#[test]
fn inset_against_container_field_list_is_false() {
    // Field `tags` is a List; `$in` against a container is non-sensical and
    // must be false (the field is "absent" from a scalar-probe POV). Both
    // the InSet fast path and the In slow path agree.
    let interner = Interner::new();
    let record = make_list_record(&interner); // {name: "Test", tags: ["rust","db","query"]}
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // All-literal values → compiles to InSet (fast path).
    let filter_in = Filter::In {
        field: vec!["tags".to_string()],
        values: vec![FilterValue::String("rust".to_string())],
    };
    let node = compile_filter(&filter_in, &interner);
    assert!(
        matches!(
            node,
            crate::query::filter::filter_node::FilterNode::InSet { .. }
        ),
        "all-literal $in must compile to InSet"
    );
    assert!(
        !node.matches(&record, &ctx),
        "$in against a List field must be false (InSet treats non-scalar as absent)"
    );
}

#[test]
fn inset_against_container_field_list_nin_is_true() {
    // Negated form: `$nin` against a List field → true (negate of absent).
    let interner = Interner::new();
    let record = make_list_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter_nin = Filter::NotIn {
        field: vec!["tags".to_string()],
        values: vec![FilterValue::String("rust".to_string())],
    };
    let node = compile_filter(&filter_nin, &interner);
    assert!(
        matches!(
            node,
            crate::query::filter::filter_node::FilterNode::InSet { .. }
        ),
        "all-literal $nin must compile to InSet"
    );
    assert!(
        node.matches(&record, &ctx),
        "$nin against a List field must be true (InSet treats non-scalar as absent → negate)"
    );
}

#[test]
fn inset_against_container_field_set_is_false() {
    // Field `roles` is a Set — same absent semantics as List.
    let interner = Interner::new();
    let record = make_set_record(&interner); // {name: "Test", roles: {admin, user}}
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter_in = Filter::In {
        field: vec!["roles".to_string()],
        values: vec![FilterValue::String("admin".to_string())],
    };
    let node = compile_filter(&filter_in, &interner);
    assert!(
        matches!(
            node,
            crate::query::filter::filter_node::FilterNode::InSet { .. }
        ),
        "all-literal $in must compile to InSet"
    );
    assert!(
        !node.matches(&record, &ctx),
        "$in against a Set field must be false (InSet treats non-scalar as absent)"
    );
}

#[test]
fn inset_and_in_agree_on_container_field() {
    // The WHOLE point of F6: InSet (all-literal fast path) and In (dynamic
    // path with at least one non-literal) must agree on a container field.
    // We force the In slow path by mixing a literal with a FieldRef that
    // resolves to the same literal value; against a List field, both nodes
    // return false for `$in`.
    let interner = Interner::new();
    let record = make_list_record(&interner); // {name: "Test", tags: [...]}
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // InSet fast path (all literals).
    let inset_node = compile_filter(
        &Filter::In {
            field: vec!["tags".to_string()],
            values: vec![FilterValue::String("rust".to_string())],
        },
        &interner,
    );
    // In slow path (one non-literal FieldRef → does not collapse to InSet).
    // The FieldRef resolves to `name` = "Test" (a scalar), which won't be in
    // the literal set anyway; what matters is the FIELD side (`tags`) is a
    // List, so scalar_at returns None and both nodes treat it as absent.
    let in_node = compile_filter(
        &Filter::In {
            field: vec!["tags".to_string()],
            values: vec![
                FilterValue::String("rust".to_string()),
                FilterValue::FieldRef {
                    path: vec!["name".to_string()],
                },
            ],
        },
        &interner,
    );
    assert!(
        matches!(
            inset_node,
            crate::query::filter::filter_node::FilterNode::InSet { .. }
        ),
        "all-literal $in must compile to InSet"
    );
    assert!(
        matches!(
            in_node,
            crate::query::filter::filter_node::FilterNode::In { .. }
        ),
        "mixed $in must compile to In (slow path)"
    );
    // Both must agree: a List field is absent for scalar probing → no match.
    assert_eq!(
        inset_node.matches(&record, &ctx),
        in_node.matches(&record, &ctx),
        "InSet and In must agree on a container (List) field"
    );
    assert!(
        !inset_node.matches(&record, &ctx),
        "both must be false for $in against a List field"
    );
}

// ============================================================================
