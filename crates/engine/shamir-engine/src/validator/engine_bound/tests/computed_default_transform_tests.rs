use shamir_query_types::filter::{FilterValue, FnCall};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::function::builtin_scalars;
use crate::table::write_helpers::apply_transforms;
use crate::validator::TransformSpec;

// ── ③.2c: computed-default expression evaluated via apply_transforms ──────────

#[test]
fn computed_default_fn_call_stamps_absent_field() {
    // strings/upper("hello") → "HELLO"
    let expr = FilterValue::FnCall {
        call: FnCall::complex(
            "strings/upper",
            vec![FilterValue::String("hello".to_string())],
        ),
    };

    let mut m = new_map();
    m.insert("name".to_string(), QueryValue::Str("alice".into()));
    let mut rec = QueryValue::Map(m);

    let transforms = vec![(
        vec!["tag".to_string()],
        TransformSpec::ComputedDefault(expr),
    )];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), 0, true);

    // The computed default should have stamped "HELLO" into the absent "tag" field.
    let tag = match &rec {
        QueryValue::Map(m) => m.get("tag").cloned(),
        _ => None,
    };
    assert_eq!(
        tag,
        Some(QueryValue::Str("HELLO".to_string())),
        "computed default expr should evaluate strings/upper('hello') → 'HELLO'"
    );
}

#[test]
fn computed_default_does_not_overwrite_present_value() {
    let expr = FilterValue::FnCall {
        call: FnCall::complex(
            "strings/upper",
            vec![FilterValue::String("hello".to_string())],
        ),
    };

    let mut m = new_map();
    m.insert("tag".to_string(), QueryValue::Str("existing".into()));
    let mut rec = QueryValue::Map(m);

    let transforms = vec![(
        vec!["tag".to_string()],
        TransformSpec::ComputedDefault(expr),
    )];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), 0, true);

    // Present value must NOT be overwritten.
    let tag = match &rec {
        QueryValue::Map(m) => m.get("tag").cloned(),
        _ => None,
    };
    assert_eq!(
        tag,
        Some(QueryValue::Str("existing".to_string())),
        "computed default must not overwrite a present value"
    );
}

#[test]
fn computed_default_does_not_overwrite_explicit_null() {
    let expr = FilterValue::FnCall {
        call: FnCall::simple("strings/upper"),
    };

    let mut m = new_map();
    m.insert("tag".to_string(), QueryValue::Null);
    let mut rec = QueryValue::Map(m);

    let transforms = vec![(
        vec!["tag".to_string()],
        TransformSpec::ComputedDefault(expr),
    )];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), 0, true);

    // Explicit Null is present → absence check fails → no stamp.
    let tag = match &rec {
        QueryValue::Map(m) => m.get("tag").cloned(),
        _ => None,
    };
    assert_eq!(
        tag,
        Some(QueryValue::Null),
        "computed default must not overwrite explicit Null (keystone invariant)"
    );
}
