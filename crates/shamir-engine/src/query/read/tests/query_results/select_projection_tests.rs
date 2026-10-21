//! Tests for SelectProjection::project_value.
//!
//! Verifies that `project_value` produces the correct key-value pairs
//! for select-all and explicit field projections.
//!
//! The old parity tests (comparing `project` against `project_value`)
//! have been replaced with concrete expected-value assertions after `project`
//! was removed in J1 elimination.

use std::sync::Arc;

use shamir_funclib::registry::FnEntry;
use shamir_funclib::registry::ScalarResult;
use shamir_funclib::scalar_resolver::{ScalarResolver, UserScalarLayer};
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::{InnerValue, QueryValue};

use shamir_query_types::read::{SelectExpr, SelectExprValue};

use crate::query::filter::{Cond, Filter, FilterValue};
use crate::query::read::select_projection::SelectProjection;
use crate::query::read::{Select, SelectItem};

/// Build an InnerValue::Map with the given string keys, interning them into
/// `interner`, and associate the provided values.
fn make_record(interner: &Interner, fields: Vec<(&str, InnerValue)>) -> InnerValue {
    let mut m = new_map_wc(fields.len());
    for (k, v) in fields {
        let key = interner.touch_ind(k).expect("intern key").into_key();
        m.insert(key, v);
    }
    InnerValue::Map(m)
}

/// SELECT * via project_value returns all fields.
#[test]
fn project_value_select_all_returns_all_fields() {
    let interner = Arc::new(Interner::new());
    let record = make_record(
        &interner,
        vec![
            ("name", InnerValue::Str("Alice".to_string())),
            ("age", InnerValue::Int(30)),
            ("active", InnerValue::Bool(true)),
        ],
    );

    let select = Select::all();
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("name"), Some(&QueryValue::Str("Alice".to_string())));
            assert_eq!(m.get("age"), Some(&QueryValue::Int(30)));
            assert_eq!(m.get("active"), Some(&QueryValue::Bool(true)));
            assert_eq!(m.len(), 3);
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval),
    }
}

/// Explicit field projection returns only the named fields.
#[test]
fn project_value_field_projection_returns_named_fields_only() {
    let interner = Arc::new(Interner::new());
    let record = make_record(
        &interner,
        vec![
            ("name", InnerValue::Str("Bob".to_string())),
            ("age", InnerValue::Int(25)),
            ("score", InnerValue::F64(9.5)),
        ],
    );

    let select = Select {
        items: vec![
            SelectItem::Field {
                path: vec!["name".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["age".to_string()],
                alias: Some("years".to_string()),
            },
        ],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            // "name" is projected as-is
            assert_eq!(m.get("name"), Some(&QueryValue::Str("Bob".to_string())));
            // "age" is projected with alias "years"
            assert_eq!(m.get("years"), Some(&QueryValue::Int(25)));
            // "age" key itself is absent (aliased)
            assert!(
                !m.contains_key("age"),
                "original key should not appear when aliased"
            );
            // "score" is not in the select list
            assert!(
                !m.contains_key("score"),
                "non-selected field should be absent"
            );
            assert_eq!(m.len(), 2);
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval),
    }
}

/// Missing field in projection results in QueryValue::Null.
#[test]
fn project_value_missing_field_is_null() {
    let interner = Arc::new(Interner::new());
    let record = make_record(
        &interner,
        vec![("name", InnerValue::Str("Carol".to_string()))],
    );

    let select = Select {
        items: vec![
            SelectItem::Field {
                path: vec!["name".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["nonexistent".to_string()],
                alias: None,
            },
        ],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("name"), Some(&QueryValue::Str("Carol".to_string())));
            assert_eq!(m.get("nonexistent"), Some(&QueryValue::Null));
        }
        _ => panic!("expected QueryValue::Map"),
    }
}

/// Empty select (no items) returns QueryValue::Map with all fields (is_all path).
#[test]
fn project_value_empty_items_returns_all() {
    let interner = Arc::new(Interner::new());
    let record = make_record(
        &interner,
        vec![("x", InnerValue::Int(1)), ("y", InnerValue::Int(2))],
    );

    let select = Select {
        items: vec![],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("x"), Some(&QueryValue::Int(1)));
            assert_eq!(m.get("y"), Some(&QueryValue::Int(2)));
            assert_eq!(m.len(), 2);
        }
        _ => panic!("expected QueryValue::Map"),
    }
}

/// #665 (#643 gap) — engine-level integration test through the real
/// production seam: `SelectProjection::new` is the one production call site
/// that populates `CondCache` (via `prescan_cond_cache` walking `funcs`
/// once, at query-compile time). This test builds a `Select` with a
/// `SelectItem::Function` (`strings/upper`) whose sole arg embeds a
/// `FilterValue::Cond` (`score > 50 ? "high" : "low"`), builds
/// `SelectProjection::new` ONCE (populating `funcs_cond_cache` internally),
/// then calls `project_value` for TWO records with differing `score`
/// values on that SAME projection instance. If the cache-hit branch inside
/// `resolve_filter_query`'s `Cond` arm ever regressed to a frozen/stale
/// answer (baked in at whichever record first hit the cache), the SECOND
/// record's assertion below would incorrectly still see "HIGH" — proving
/// the SAME internal `funcs_cond_cache`, built once, correctly serves both
/// calls with per-record-correct answers.
#[test]
fn project_value_cond_function_projection_caches_and_evaluates_per_record() {
    let interner = Arc::new(Interner::new());
    // `SelectProjection::new`'s prescan compiles the $cond's condition
    // immediately via `compile_filter`, whose field-path resolution
    // (`intern_field_path_compact` → `Interner::get_ind`) is lookup-only —
    // it never inserts. "score" must already be interned before `new()`
    // runs, or the compiled field path fails to resolve and the node folds
    // to `FilterNode::False` (always "low"), independent of any record.
    // Mirrors production: field names are interned by the write path before
    // a query ever compiles a filter that references them.
    interner.touch_ind("score").unwrap();

    let cond_arg = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["score".to_string()],
                value: FilterValue::Int(50),
            },
            FilterValue::String("high".to_string()),
            FilterValue::String("low".to_string()),
        )),
    };

    let select = Select {
        items: vec![SelectItem::Function {
            name: "strings/upper".to_string(),
            args: vec![cond_arg],
            alias: Some("bucket".to_string()),
        }],
        distinct: false,
    };

    // Built ONCE — this is what populates `funcs_cond_cache` internally via
    // `prescan_cond_cache`, the real production call site under test.
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();

    let record_high = make_record(&interner, vec![("score", InnerValue::Int(80))]);
    let record_low = make_record(&interner, vec![("score", InnerValue::Int(20))]);

    // SAME `proj` (SAME internal cond cache) serves both calls.
    let qval_high = proj.project_value(&record_high, &interner);
    let qval_low = proj.project_value(&record_low, &interner);

    match &qval_high {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("bucket"),
                Some(&QueryValue::Str("HIGH".to_string())),
                "score=80 must project through the cached $cond to \"high\" \
                 (upper-cased by strings/upper)"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval_high),
    }
    match &qval_low {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("bucket"),
                Some(&QueryValue::Str("LOW".to_string())),
                "score=20 must project through the SAME cached $cond to \"low\" — \
                 proving the SAME funcs_cond_cache, built once, is genuinely \
                 re-evaluated per record rather than frozen to the first call's answer"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval_low),
    }
}

// ============================================================================
// Fix 2 (Finding 8) — user-registered scalars available in SELECT projections
// ============================================================================

/// Build a ScalarResolver with a user-registered scalar `my_double` that
/// doubles its Int argument.
fn resolver_with_user_scalar() -> ScalarResolver {
    let layer = UserScalarLayer::new();
    layer.register(
        "my_double",
        FnEntry::pure(
            |args: &[QueryValue]| -> ScalarResult {
                match &args[0] {
                    QueryValue::Int(n) => Ok(QueryValue::Int(n * 2)),
                    _ => Err(shamir_funclib::registry::ScalarError::new("type_mismatch")),
                }
            },
            1,
            Some(1),
        ),
    );
    ScalarResolver::new(std::sync::Arc::new(layer))
}

#[test]
fn project_value_user_scalar_resolves_through_projection() {
    // Site 5: SELECT projection scalar-function must resolve user-registered
    // scalars. Before Fix 2, `SelectProjection::new` built a builtins-only
    // FilterContext, so `$fn: my_double` silently fell back to "unknown
    // function" → Null. After Fix 2, the resolver is threaded through.
    let interner = Interner::new();
    interner.touch_ind("n").unwrap();

    let select = Select {
        items: vec![SelectItem::Function {
            name: "my_double".to_string(),
            args: vec![FilterValue::field_ref("n")],
            alias: Some("doubled".to_string()),
        }],
        distinct: false,
    };

    let resolver = resolver_with_user_scalar();
    let proj = SelectProjection::new(&select, &interner, resolver).unwrap();

    let record = make_record(&interner, vec![("n", InnerValue::Int(21))]);
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("doubled"),
                Some(&QueryValue::Int(42)),
                "user-registered scalar 'my_double' must resolve in SELECT projection"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval),
    }
}

#[test]
fn project_value_builtins_only_still_works() {
    // Regression: builtins-only resolver (no user scalars) still works
    // for built-in scalar functions.
    let interner = Interner::new();
    interner.touch_ind("s").unwrap();

    let select = Select {
        items: vec![SelectItem::Function {
            name: "strings/upper".to_string(),
            args: vec![FilterValue::field_ref("s")],
            alias: Some("upper_s".to_string()),
        }],
        distinct: false,
    };

    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();

    let record = make_record(&interner, vec![("s", InnerValue::Str("hello".into()))]);
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("upper_s"), Some(&QueryValue::Str("HELLO".into())));
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval),
    }
}

// ============================================================================
// #1024 — `SelectItem::Expression` evaluation (replaces the old
// `select_expression_not_supported` rejection).
// ============================================================================

/// Arithmetic `SelectExpr` over real field data: `SELECT (price * qty) AS
/// total` computes the product from the record's own fields, proving the
/// translated `FilterValue::Expr` tree resolves `FieldRef`s against the
/// actual row via the SAME `resolve_filter_query` pipeline
/// `SelectItem::Function` uses.
#[test]
fn project_value_expression_arithmetic_over_field_data() {
    let interner = Arc::new(Interner::new());
    let record = make_record(
        &interner,
        vec![("price", InnerValue::Int(3)), ("qty", InnerValue::Int(4))],
    );

    let select = Select {
        items: vec![SelectItem::Expression {
            expr: SelectExpr::Mul {
                left: Box::new(SelectExpr::Field {
                    path: vec!["price".to_string()],
                }),
                right: Box::new(SelectExpr::Field {
                    path: vec!["qty".to_string()],
                }),
            },
            alias: Some("total".to_string()),
        }],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("total"),
                Some(&QueryValue::Int(12)),
                "price(3) * qty(4) must compute to 12 via the translated FilterExpr"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval),
    }
}

/// Literal-only `SelectExpr` (no field reference at all) — proves the
/// translation and evaluation work even when there is nothing to resolve
/// from the record.
#[test]
fn project_value_expression_literal_only() {
    let interner = Arc::new(Interner::new());
    let record = make_record(&interner, vec![("name", InnerValue::Str("x".into()))]);

    let select = Select {
        items: vec![SelectItem::Expression {
            expr: SelectExpr::Add {
                left: Box::new(SelectExpr::Literal {
                    value: SelectExprValue::Int(2),
                }),
                right: Box::new(SelectExpr::Literal {
                    value: SelectExprValue::Int(3),
                }),
            },
            alias: Some("five".to_string()),
        }],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("five"), Some(&QueryValue::Int(5)));
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval),
    }
}

/// No alias given — the output key defaults to the literal `"expr"` (there
/// is no natural per-item name like `SelectItem::Function`'s `name` field).
#[test]
fn project_value_expression_without_alias_defaults_to_expr_key() {
    let interner = Arc::new(Interner::new());
    let record = make_record(&interner, vec![]);

    let select = Select {
        items: vec![SelectItem::Expression {
            expr: SelectExpr::Literal {
                value: SelectExprValue::Int(7),
            },
            alias: None,
        }],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("expr"), Some(&QueryValue::Int(7)));
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval),
    }
}

/// Proves the OLD rejection is genuinely gone: `SelectProjection::new` used
/// to return `Err(DbError::Validation("select_expression_not_supported"))`
/// for ANY `SelectItem::Expression`, including the trivial literal shape
/// used by the old reject tests. Constructing the SAME shape now succeeds.
#[test]
fn project_value_expression_no_longer_rejected() {
    let interner = Arc::new(Interner::new());
    let select = Select {
        items: vec![SelectItem::Expression {
            expr: SelectExpr::Literal {
                value: SelectExprValue::Int(1),
            },
            alias: Some("bogus".to_string()),
        }],
        distinct: false,
    };
    let result = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only());
    assert!(
        result.is_ok(),
        "SelectItem::Expression must no longer be rejected by SelectProjection::new, got {:?}",
        result.err()
    );
}

// ============================================================================
// #1069 Defect 1 — `SELECT *` combined with other items must error, not
// silently drop the extras.
// ============================================================================

/// Before the fix: `[All, Expression(2+3 AS five)]` silently returned just
/// the raw record — `is_all` went true because SOME item was `All`, and
/// `fields`/`funcs` (including the `five` expression) were discarded
/// wholesale, with no error at all. Must now be a validation error.
#[test]
fn select_star_combined_with_expression_is_rejected() {
    let interner = Arc::new(Interner::new());
    let select = Select {
        items: vec![
            SelectItem::All,
            SelectItem::Expression {
                expr: SelectExpr::Add {
                    left: Box::new(SelectExpr::Literal {
                        value: SelectExprValue::Int(2),
                    }),
                    right: Box::new(SelectExpr::Literal {
                        value: SelectExprValue::Int(3),
                    }),
                },
                alias: Some("five".to_string()),
            },
        ],
        distinct: false,
    };
    let result = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only());
    match result {
        Err(shamir_storage::error::DbError::Validation(_)) => {}
        other => panic!(
            "SELECT * combined with another item must be rejected with a \
             Validation error, not silently drop the extra item; got {}",
            match other {
                Ok(_) => "Ok(..)".to_string(),
                Err(e) => format!("Err({e})"),
            }
        ),
    }
}

/// Same rejection for `[All, Field]` — the defect applied to ANY extra
/// item alongside `*`, not just `Expression`.
#[test]
fn select_star_combined_with_field_is_rejected() {
    let interner = Arc::new(Interner::new());
    let select = Select {
        items: vec![
            SelectItem::All,
            SelectItem::Field {
                path: vec!["name".to_string()],
                alias: None,
            },
        ],
        distinct: false,
    };
    let result = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only());
    assert!(
        result.is_err(),
        "SELECT * combined with a field must be rejected"
    );
}

/// Regression guard: `*` alone (the normal, common case) still works.
#[test]
fn select_star_alone_still_works() {
    let interner = Arc::new(Interner::new());
    let record = make_record(&interner, vec![("x", InnerValue::Int(1))]);
    let select = Select {
        items: vec![SelectItem::All],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);
    match &qval {
        QueryValue::Map(m) => assert_eq!(m.get("x"), Some(&QueryValue::Int(1))),
        _ => panic!("expected QueryValue::Map"),
    }
}

// ============================================================================
// #1069 Defect 2 — colliding output column names must error, not
// silently last-write-wins.
// ============================================================================

/// Before the fix: two unaliased `SelectItem::Expression` items both
/// defaulted to the literal key `"expr"`, so the second silently overwrote
/// the first in the output map. Must now be a validation error.
#[test]
fn two_unaliased_expressions_collide_and_are_rejected() {
    let interner = Arc::new(Interner::new());
    let one = || SelectExpr::Literal {
        value: SelectExprValue::Int(1),
    };
    let select = Select {
        items: vec![
            SelectItem::Expression {
                expr: one(),
                alias: None,
            },
            SelectItem::Expression {
                expr: one(),
                alias: None,
            },
        ],
        distinct: false,
    };
    let result = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only());
    match result {
        Err(shamir_storage::error::DbError::Validation(_)) => {}
        other => panic!(
            "two unaliased expressions colliding on the default 'expr' key must be \
             rejected with a Validation error, not silently last-write-wins; got {}",
            match other {
                Ok(_) => "Ok(..)".to_string(),
                Err(e) => format!("Err({e})"),
            }
        ),
    }
}

/// A field and a function colliding on an EXPLICIT alias must also be
/// rejected — the collision check covers all item kinds, not just the
/// `"expr"` default.
#[test]
fn field_and_function_colliding_on_explicit_alias_is_rejected() {
    let interner = Arc::new(Interner::new());
    let select = Select {
        items: vec![
            SelectItem::Field {
                path: vec!["name".to_string()],
                alias: Some("out".to_string()),
            },
            SelectItem::Function {
                name: "strings/upper".to_string(),
                args: vec![FilterValue::field_ref("name")],
                alias: Some("out".to_string()),
            },
        ],
        distinct: false,
    };
    let result = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only());
    assert!(
        result.is_err(),
        "a field and a function colliding on the same explicit alias must be rejected"
    );
}

/// Regression guard: distinct aliases for two otherwise-identical
/// expressions still work fine.
#[test]
fn two_expressions_with_distinct_aliases_both_survive() {
    let interner = Arc::new(Interner::new());
    let record = make_record(&interner, vec![]);
    let select = Select {
        items: vec![
            SelectItem::Expression {
                expr: SelectExpr::Literal {
                    value: SelectExprValue::Int(1),
                },
                alias: Some("a".to_string()),
            },
            SelectItem::Expression {
                expr: SelectExpr::Literal {
                    value: SelectExprValue::Int(2),
                },
                alias: Some("b".to_string()),
            },
        ],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);
    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("a"), Some(&QueryValue::Int(1)));
            assert_eq!(m.get("b"), Some(&QueryValue::Int(2)));
        }
        _ => panic!("expected QueryValue::Map"),
    }
}

// ============================================================================
// #1069 Defect 3 — documented (not accidental) Null-collapse semantics for
// evaluation failures in SELECT projection.
// ============================================================================

/// Division by zero in a SELECT expression must produce `Null` — this is
/// now a DOCUMENTED contract (see `project_value`'s doc comment on the
/// `funcs` loop), matching the SAME silent-miss semantics
/// `resolve_filter_query` already applies uniformly to WHERE/`when`/
/// `for_each`, not an accident specific to this one operator.
#[test]
fn division_by_zero_in_select_expression_produces_documented_null() {
    let interner = Arc::new(Interner::new());
    let record = make_record(&interner, vec![]);
    let select = Select {
        items: vec![SelectItem::Expression {
            expr: SelectExpr::Div {
                left: Box::new(SelectExpr::Literal {
                    value: SelectExprValue::Int(10),
                }),
                right: Box::new(SelectExpr::Literal {
                    value: SelectExprValue::Int(0),
                }),
            },
            alias: Some("result".to_string()),
        }],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);
    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("result"),
                Some(&QueryValue::Null),
                "division by zero must produce the documented Null (not an error, not \
                 a crash, not a raw float Infinity)"
            );
        }
        _ => panic!("expected QueryValue::Map"),
    }
}

/// An erroring scalar function call (unknown function name) must ALSO
/// produce `Null` in the projection output — same collapse point as
/// division by zero, both routed through `resolve_filter_query`'s uniform
/// `None`-on-any-failure contract.
#[test]
fn unknown_scalar_function_in_select_produces_documented_null() {
    let interner = Arc::new(Interner::new());
    let record = make_record(&interner, vec![("n", InnerValue::Int(1))]);
    let select = Select {
        items: vec![SelectItem::Function {
            name: "this_function_does_not_exist".to_string(),
            args: vec![FilterValue::field_ref("n")],
            alias: Some("result".to_string()),
        }],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only()).unwrap();
    let qval = proj.project_value(&record, &interner);
    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("result"), Some(&QueryValue::Null));
        }
        _ => panic!("expected QueryValue::Map"),
    }
}
