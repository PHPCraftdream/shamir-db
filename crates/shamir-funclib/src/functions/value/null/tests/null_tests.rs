//! Per-function `/null` tests — at least one correct-result assert and one
//! edge/error case per registered function, plus a wiring smoke-test that
//! resolves the folder-qualified name through the top-level registry.

use crate::null;
use crate::register_builtins;
use crate::registry::{v_bool, ScalarRegistry};
use rust_decimal::Decimal;
use shamir_types::types::common::{TMap, TSet};
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    null::register(&mut r);
    r
}

#[test]
fn coalesce_first_non_null_wins() {
    let r = reg();
    // First-non-null among 3+ args (position 2 wins).
    assert_eq!(
        r.call(
            "coalesce",
            &[
                QueryValue::Null,
                QueryValue::Null,
                QueryValue::Int(5),
                QueryValue::Int(9)
            ]
        )
        .unwrap(),
        QueryValue::Int(5)
    );
    // First arg non-null short-circuits immediately.
    assert_eq!(
        r.call("coalesce", &[QueryValue::Str("x".into()), QueryValue::Null])
            .unwrap(),
        QueryValue::Str("x".into())
    );
}

#[test]
fn coalesce_single_arg_passthrough() {
    let r = reg();
    // Non-null single arg passes through unchanged.
    assert_eq!(
        r.call("coalesce", &[QueryValue::Int(42)]).unwrap(),
        QueryValue::Int(42)
    );
    // Null single arg yields Null (NOT an error).
    assert_eq!(
        r.call("coalesce", &[QueryValue::Null]).unwrap(),
        QueryValue::Null
    );
}

#[test]
fn coalesce_all_null_is_null_not_error() {
    let r = reg();
    // All-null among many args → Null (a valid, common case, unlike min/max's
    // empty-arg-list error).
    assert_eq!(
        r.call(
            "coalesce",
            &[QueryValue::Null, QueryValue::Null, QueryValue::Null]
        )
        .unwrap(),
        QueryValue::Null
    );
}

#[test]
fn coalesce_zero_args_is_arity() {
    let r = reg();
    // Zero args rejected by the registry's min-args gate (min_args = 1),
    // mirroring math::min/max's empty-arg-list behaviour.
    assert_eq!(r.call("coalesce", &[]).unwrap_err().code, "arity");
}

#[test]
fn if_null_non_null_returns_v() {
    let r = reg();
    // Non-null v returned unchanged (default ignored).
    assert_eq!(
        r.call("if_null", &[QueryValue::Int(7), QueryValue::Int(0)])
            .unwrap(),
        QueryValue::Int(7)
    );
    assert_eq!(
        r.call(
            "if_null",
            &[QueryValue::Str("hi".into()), QueryValue::Str("def".into())]
        )
        .unwrap(),
        QueryValue::Str("hi".into())
    );
}

#[test]
fn if_null_null_returns_default() {
    let r = reg();
    // Null v → default.
    assert_eq!(
        r.call("if_null", &[QueryValue::Null, QueryValue::Int(3)])
            .unwrap(),
        QueryValue::Int(3)
    );
    // default itself may be Null (no special casing).
    assert_eq!(
        r.call("if_null", &[QueryValue::Null, QueryValue::Null])
            .unwrap(),
        QueryValue::Null
    );
}

#[test]
fn if_null_arity() {
    let r = reg();
    // Exactly 2 args required.
    assert_eq!(
        r.call("if_null", &[QueryValue::Null]).unwrap_err().code,
        "arity"
    );
    assert_eq!(
        r.call(
            "if_null",
            &[QueryValue::Null, QueryValue::Null, QueryValue::Null]
        )
        .unwrap_err()
        .code,
        "arity"
    );
}

#[test]
fn nullif_equal_yields_null() {
    let r = reg();
    // Same-variant equality → Null.
    assert_eq!(
        r.call("nullif", &[QueryValue::Int(5), QueryValue::Int(5)])
            .unwrap(),
        QueryValue::Null
    );
}

#[test]
fn nullif_cross_type_equal_via_compare() {
    let r = reg();
    // Int(5) vs Dec(5.0) compare Equal under the cross-type total order —
    // PartialEq would miss this, compare catches it.
    assert_eq!(
        r.call(
            "nullif",
            &[QueryValue::Int(5), QueryValue::Dec(Decimal::from(5))]
        )
        .unwrap(),
        QueryValue::Null
    );
    // Int(5) vs F64(5.0) likewise.
    assert_eq!(
        r.call("nullif", &[QueryValue::Int(5), QueryValue::F64(5.0)])
            .unwrap(),
        QueryValue::Null
    );
}

#[test]
fn nullif_unequal_returns_a_unchanged() {
    let r = reg();
    // Not equal → return a unchanged (not b, not coerced).
    assert_eq!(
        r.call("nullif", &[QueryValue::Int(5), QueryValue::Int(6)])
            .unwrap(),
        QueryValue::Int(5)
    );
    // Cross-type unequal: Int(5) vs Str("5") compare by rank (Int rank 2 <
    // Str rank 3), so NOT equal → a returned.
    assert_eq!(
        r.call("nullif", &[QueryValue::Int(5), QueryValue::Str("5".into())])
            .unwrap(),
        QueryValue::Int(5)
    );
}

#[test]
fn nullif_arity() {
    let r = reg();
    assert_eq!(
        r.call("nullif", &[QueryValue::Int(1)]).unwrap_err().code,
        "arity"
    );
    assert_eq!(
        r.call(
            "nullif",
            &[QueryValue::Int(1), QueryValue::Int(1), QueryValue::Int(1)]
        )
        .unwrap_err()
        .code,
        "arity"
    );
}

#[test]
fn is_null_true_for_null() {
    let r = reg();
    assert_eq!(
        r.call("is_null", &[QueryValue::Null]).unwrap(),
        v_bool(true)
    );
}

#[test]
fn is_null_false_for_every_other_variant() {
    let r = reg();
    // Enumerate a representative value for every non-null variant.
    let non_null = [
        QueryValue::Int(1),
        QueryValue::Str("x".into()),
        QueryValue::Bool(true),
        QueryValue::List(vec![QueryValue::Int(1)]),
        QueryValue::Dec(Decimal::from(1)),
        QueryValue::Big(num_bigint::BigInt::from(1)),
        QueryValue::F64(1.0),
        QueryValue::Bin(vec![1]),
    ];
    for v in &non_null {
        assert_eq!(
            r.call("is_null", std::slice::from_ref(v)).unwrap(),
            v_bool(false),
            "is_null({v:?}) should be false"
        );
    }
    // Set / Map need the shamir-types constructors; cover them via the
    // empty-container case (still non-null).
    let empty_set: TSet<QueryValue> = TSet::default();
    let empty_map: TMap<String, QueryValue> = TMap::default();
    assert_eq!(
        r.call("is_null", &[QueryValue::Set(empty_set)]).unwrap(),
        v_bool(false)
    );
    assert_eq!(
        r.call("is_null", &[QueryValue::Map(empty_map)]).unwrap(),
        v_bool(false)
    );
}

#[test]
fn is_null_arity() {
    let r = reg();
    assert_eq!(r.call("is_null", &[]).unwrap_err().code, "arity");
    assert_eq!(
        r.call("is_null", &[QueryValue::Null, QueryValue::Null])
            .unwrap_err()
            .code,
        "arity"
    );
}

#[test]
fn register_builtins_exposes_null_folder_qualified_names() {
    // Regression: register_builtins() wires the null category under the
    // `null/` folder prefix (matching math/abs, value_nav/type_of, etc.),
    // with no duplicate-registration panic.
    let reg = register_builtins();
    for name in [
        "null/coalesce",
        "null/if_null",
        "null/nullif",
        "null/is_null",
    ] {
        assert!(
            reg.get(name).is_some(),
            "expected `{name}` in register_builtins()"
        );
    }
    // Smoke-test: dispatch coalesce through the top-level registry.
    assert_eq!(
        reg.call("null/coalesce", &[QueryValue::Null, QueryValue::Int(5)])
            .unwrap(),
        QueryValue::Int(5)
    );
}
