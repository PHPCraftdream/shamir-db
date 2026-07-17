//! #667 — pins the 3-way "nothing to compare" semantics of
//! `FilterNode::ValueCompare::matches` (compiled from `Filter::ValueCompare`)
//! as documented on the variant itself (`filter_node.rs`) and on
//! `compare_values` (`resolve.rs:81`).
//!
//! `matches()` resolves `left`/`right` independently via
//! `resolve_filter_query` and then branches on
//! `(Option<QueryValue>, Option<QueryValue>)`. There are THREE distinct
//! "nothing to compare" shapes, only one of which is genuinely comparable:
//!
//! 1. A genuinely unresolvable operand (either or both sides) — the outer
//!    `(None, _) | (_, None)` arm — makes only `Ne` true.
//! 2. BOTH sides resolve to the literal value `null` — reaches the inner
//!    `(Some(a), Some(b))` arm and `compare_values(&Null, &Null)` returns
//!    `Some(Equal)` deliberately — makes `Eq`/`Gte`/`Lte` true (the OPPOSITE
//!    of case 1).
//! 3. ONE side resolves to literal `null`, the other to a non-null value of
//!    a different type — also reaches the inner arm, but `compare_values`
//!    falls through its `_ => None` catch-all — makes only `Ne` true, same
//!    outward shape as case 1 but via a type-mismatch, not an absent
//!    operand.
//!
//! This file exists specifically because, before #667, NOTHING exercised
//! any `Null`/unresolvable operand on either side of `ValueCompare` — only
//! ordinary same-type numeric comparisons were covered
//! (`when_skip_tests.rs`'s `value_compare_makes_balance_gte_amount_scenario_work_*`).

use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_query_types::filter::ValueCompareOp;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use super::helpers::empty_refs;

/// Empty synthetic record — `ValueCompare` has no field/record dependency,
/// so an empty map is sufficient for every test in this file (mirrors how
/// `when_skip_tests.rs` treats `ValueCompare` guards against an empty
/// synthetic record).
fn empty_record() -> InnerValue {
    InnerValue::Map(new_map())
}

fn build_and_match(left: FilterValue, cmp: ValueCompareOp, right: FilterValue) -> bool {
    let interner = Interner::new();
    let record = empty_record();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ValueCompare { left, cmp, right };
    let node = compile_filter(&filter, &interner);
    node.matches(&record, &ctx)
}

// ---------------------------------------------------------------------
// Case 1: genuinely unresolvable, BOTH sides (unbound `$param` on both).
// ---------------------------------------------------------------------

#[test]
fn both_sides_unresolvable_param_eq_is_false() {
    let left = FilterValue::Param {
        name: "left_unbound".to_string(),
    };
    let right = FilterValue::Param {
        name: "right_unbound".to_string(),
    };
    assert!(
        !build_and_match(left, ValueCompareOp::Eq, right),
        "both operands genuinely unresolvable ($param unbound on both \
         sides) -> Eq must be false"
    );
}

#[test]
fn both_sides_unresolvable_param_ne_is_true() {
    let left = FilterValue::Param {
        name: "left_unbound".to_string(),
    };
    let right = FilterValue::Param {
        name: "right_unbound".to_string(),
    };
    assert!(
        build_and_match(left, ValueCompareOp::Ne, right),
        "both operands genuinely unresolvable ($param unbound on both \
         sides) -> Ne must be true (only Ne matches the (None, _) | (_, \
         None) arm)"
    );
}

#[test]
fn both_sides_unresolvable_param_gte_lte_are_false() {
    let mk = || {
        (
            FilterValue::Param {
                name: "left_unbound".to_string(),
            },
            FilterValue::Param {
                name: "right_unbound".to_string(),
            },
        )
    };
    let (l, r) = mk();
    assert!(
        !build_and_match(l, ValueCompareOp::Gte, r),
        "both operands unresolvable -> Gte must be false"
    );
    let (l, r) = mk();
    assert!(
        !build_and_match(l, ValueCompareOp::Lte, r),
        "both operands unresolvable -> Lte must be false"
    );
}

// ---------------------------------------------------------------------
// Case 2 (brief item 2): genuinely unresolvable, ONE side only — proves
// the (None, _) | (_, None) arm does not distinguish left-absent from
// right-absent from both-absent; same boolean shape as case 1.
// ---------------------------------------------------------------------

#[test]
fn one_side_unresolvable_param_matches_same_shape_as_both_sides_unresolvable() {
    // left is a literal, right is an unbound $param.
    let left = FilterValue::Int(5);
    let right = FilterValue::Param {
        name: "right_unbound".to_string(),
    };
    assert!(
        !build_and_match(left.clone(), ValueCompareOp::Eq, right.clone()),
        "one side unresolvable (right) -> Eq must be false, same shape as \
         both-sides-unresolvable"
    );
    assert!(
        build_and_match(left.clone(), ValueCompareOp::Ne, right.clone()),
        "one side unresolvable (right) -> Ne must be true, same shape as \
         both-sides-unresolvable"
    );
    assert!(
        !build_and_match(left.clone(), ValueCompareOp::Gte, right.clone()),
        "one side unresolvable (right) -> Gte must be false"
    );
    assert!(
        !build_and_match(left, ValueCompareOp::Lte, right),
        "one side unresolvable (right) -> Lte must be false"
    );

    // Mirror: left is the unbound $param, right is the literal — proves the
    // arm is symmetric (neither side is special-cased).
    let left = FilterValue::Param {
        name: "left_unbound".to_string(),
    };
    let right = FilterValue::Int(5);
    assert!(
        !build_and_match(left.clone(), ValueCompareOp::Eq, right.clone()),
        "one side unresolvable (left) -> Eq must be false, same shape \
         regardless of which side is absent"
    );
    assert!(
        build_and_match(left, ValueCompareOp::Ne, right),
        "one side unresolvable (left) -> Ne must be true, same shape \
         regardless of which side is absent"
    );
}

// ---------------------------------------------------------------------
// Case 3 (brief item 3): BOTH sides literal `null` — the decisive case
// proving `null` is treated as a genuinely self-equal value, not as
// "absent". OPPOSITE Eq/Ne outcome from cases 1/2 above.
// ---------------------------------------------------------------------

#[test]
fn both_sides_literal_null_eq_is_true() {
    assert!(
        build_and_match(FilterValue::Null, ValueCompareOp::Eq, FilterValue::Null),
        "both operands are the literal value null -> Eq must be TRUE — \
         compare_values(&Null, &Null) deliberately returns Some(Equal), \
         so an explicit resolved null is a genuinely comparable, equal \
         value, unlike a genuinely unresolvable operand (case 1/2 above)"
    );
}

#[test]
fn both_sides_literal_null_ne_is_false() {
    assert!(
        !build_and_match(FilterValue::Null, ValueCompareOp::Ne, FilterValue::Null),
        "both operands are the literal value null -> Ne must be FALSE — \
         the OPPOSITE of case 1/2's Ne=true, because null vs null reaches \
         the (Some(a), Some(b)) arm, not the None-operand arm"
    );
}

#[test]
fn both_sides_literal_null_gte_lte_are_true() {
    assert!(
        build_and_match(FilterValue::Null, ValueCompareOp::Gte, FilterValue::Null),
        "both operands null -> Gte must be true (Equal counts as \
         Greater-or-Equal)"
    );
    assert!(
        build_and_match(FilterValue::Null, ValueCompareOp::Lte, FilterValue::Null),
        "both operands null -> Lte must be true (Equal counts as \
         Less-or-Equal)"
    );
}

// ---------------------------------------------------------------------
// Case 4 (brief item 4): ONE side literal null, other side a real non-null
// value of a different type — same outward Eq/Ne shape as case 1/2, but
// reached via compare_values's type-mismatch fallthrough, not the
// None-operand path. Kept as a separate test because a reader auditing
// compare_values in isolation (without the ValueCompare wrapper) needs to
// know this coincidence is intentional, not an oversight.
// ---------------------------------------------------------------------

#[test]
fn null_vs_non_null_type_mismatch_eq_is_false() {
    assert!(
        !build_and_match(FilterValue::Null, ValueCompareOp::Eq, FilterValue::Int(5)),
        "null vs Int(5) -> Eq must be false. Both operands DO resolve to \
         Some(..) (this reaches the inner (Some, Some) arm, NOT the \
         None-operand arm from case 1/2) but compare_values(&Null, &Int(5)) \
         falls through its `_ => None` catch-all since no same-type arm \
         matches -> Eq/Gte/Lte are false, Ne is true, same outward shape as \
         case 1/2 but via a completely different code path (resolved type \
         mismatch, not an absent operand)"
    );
}

#[test]
fn null_vs_non_null_type_mismatch_ne_is_true() {
    assert!(
        build_and_match(FilterValue::Null, ValueCompareOp::Ne, FilterValue::Int(5)),
        "null vs Int(5) -> Ne must be true, via compare_values returning \
         None (type mismatch), not via the None-operand arm — a distinct \
         code path from case 1/2 that happens to produce the same boolean \
         result"
    );
}

#[test]
fn null_vs_non_null_type_mismatch_gte_lte_are_false() {
    assert!(
        !build_and_match(FilterValue::Null, ValueCompareOp::Gte, FilterValue::Int(5)),
        "null vs Int(5) -> Gte must be false (compare_values returns None)"
    );
    assert!(
        !build_and_match(FilterValue::Null, ValueCompareOp::Lte, FilterValue::Int(5)),
        "null vs Int(5) -> Lte must be false (compare_values returns None)"
    );
}

// ---------------------------------------------------------------------
// Case 5 (brief item 5): regression/sanity — an ordinary same-type
// comparison still behaves as before. `when_skip_tests.rs` already covers
// this end-to-end via `value_compare_makes_balance_gte_amount_scenario_work_*`;
// this direct unit test gives a baseline right next to the null-focused
// cases above, in the same file/harness.
// ---------------------------------------------------------------------

#[test]
fn ordinary_same_type_comparison_still_works() {
    assert!(
        build_and_match(
            FilterValue::Int(100),
            ValueCompareOp::Gte,
            FilterValue::Int(40)
        ),
        "100 >= 40 must still evaluate true — sanity baseline next to the \
         null-focused cases above"
    );
    assert!(
        !build_and_match(
            FilterValue::Int(10),
            ValueCompareOp::Gte,
            FilterValue::Int(40)
        ),
        "10 >= 40 must still evaluate false"
    );
}
