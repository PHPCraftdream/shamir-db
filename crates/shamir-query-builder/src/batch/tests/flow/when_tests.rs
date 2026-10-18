//! Tests for the `Batch::when` / `Batch::switch` conditional-execution
//! builder ergonomics (Epic03/C, task #646).

use shamir_query_types::filter::{Filter, FilterValue};

use crate::batch::{Batch, BuildError};
use crate::ddl;
use crate::filter;
use crate::query::Query;
use crate::write::{self, doc};

// ============================================================================
// Batch::when — sets the field, wire-serializes as expected
// ============================================================================

#[test]
fn when_sets_field_on_entry() {
    let mut b = Batch::new();
    let flag = b.query("flag", Query::from("flags"));
    let cond = filter::eq("active", true);
    let guarded = b.insert(
        "maybe_insert",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.when(&guarded, cond.clone());
    let _ = &flag;

    let req = b.build();
    let entry = req.queries.get("maybe_insert").unwrap();
    assert_eq!(entry.when, Some(cond));
}

#[test]
fn when_wire_serialization_round_trips() {
    use crate::wire::ToWire;
    use shamir_types::types::value::QueryValue;

    let mut b = Batch::new();
    b.id(1);
    let cond = filter::eq("active", true);
    let guarded = b.insert(
        "maybe_insert",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.when(&guarded, cond);

    let qv = b.build().to_query_value().unwrap();
    let queries = qv.get("queries").expect("queries key");
    let entry = queries.get("maybe_insert").expect("entry");
    let when = entry.get("when").expect("when key present on wire");
    match when {
        QueryValue::Map(_) => {}
        other => panic!("expected Map for 'when', got {other:?}"),
    }
}

#[test]
fn when_absent_is_omitted_from_wire() {
    use crate::wire::ToWire;

    let mut b = Batch::new();
    b.insert(
        "unconditional",
        write::insert("users").row(doc().set("name", "Bob")),
    );

    let qv = b.build().to_query_value().unwrap();
    let queries = qv.get("queries").expect("queries key");
    let entry = queries.get("unconditional").expect("entry");
    assert!(
        entry.get("when").is_none(),
        "entry without a `when` guard must omit the field on the wire"
    );
}

// ============================================================================
// try_build validation — $query refs inside `when` are checked
// ============================================================================

#[test]
fn try_build_catches_unknown_alias_inside_when() {
    let mut b = Batch::new();
    let guarded = b.insert(
        "maybe_insert",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    let bad_ref = Filter::Eq {
        field: vec!["x".to_string()],
        value: FilterValue::QueryRef {
            alias: "nonexistent".to_string(),
            path: None,
        },
    };
    b.when(&guarded, bad_ref);

    let err = b.try_build().unwrap_err();
    assert!(
        matches!(
            &err,
            BuildError::UnknownAlias { alias, referenced_by }
                if alias == "nonexistent" && referenced_by == "maybe_insert"
        ),
        "expected UnknownAlias, got: {:?}",
        err
    );
}

#[test]
fn try_build_catches_self_reference_inside_when() {
    let mut b = Batch::new();
    let guarded = b.insert(
        "maybe_insert",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    let self_ref = Filter::Eq {
        field: vec!["x".to_string()],
        value: FilterValue::QueryRef {
            alias: "maybe_insert".to_string(),
            path: None,
        },
    };
    b.when(&guarded, self_ref);

    let err = b.try_build().unwrap_err();
    assert!(
        matches!(&err, BuildError::SelfReference { alias } if alias == "maybe_insert"),
        "expected SelfReference, got: {:?}",
        err
    );
}

#[test]
fn try_build_accepts_when_referencing_known_alias() {
    let mut b = Batch::new();
    let flag = b.query("flag", Query::from("flags"));
    let cond = Filter::Eq {
        field: vec!["x".to_string()],
        value: flag.first().field("active"),
    };
    let guarded = b.insert(
        "maybe_insert",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    b.when(&guarded, cond);

    assert!(b.try_build().is_ok());
}

// ============================================================================
// Batch::switch — complementary `when` filters across cases + default
// ============================================================================

#[test]
fn switch_generates_complementary_when_filters_for_two_cases_and_default() {
    let mut b = Batch::new();
    let case1 = filter::eq("tier", "vip");
    let case2 = filter::eq("tier", "regular");

    let handles = b.switch(
        vec![
            (
                "vip_email",
                case1.clone(),
                write::insert("emails").row(doc().set("kind", "vip")),
            ),
            (
                "regular_email",
                case2.clone(),
                write::insert("emails").row(doc().set("kind", "regular")),
            ),
        ],
        (
            "newbie_email",
            write::insert("emails").row(doc().set("kind", "newbie")),
        ),
    );

    assert_eq!(handles.len(), 3);
    assert_eq!(handles[0].alias(), "vip_email");
    assert_eq!(handles[1].alias(), "regular_email");
    assert_eq!(handles[2].alias(), "newbie_email");

    let req = b.build();

    // Case 1: guard == case1 condition (no priors to negate).
    let e1 = req.queries.get("vip_email").unwrap();
    assert_eq!(e1.when, Some(case1.clone()));

    // Case 2: guard == AND(NOT case1, case2).
    let e2 = req.queries.get("regular_email").unwrap();
    assert_eq!(
        e2.when,
        Some(Filter::And {
            filters: vec![
                Filter::Not {
                    filter: Box::new(case1.clone())
                },
                case2.clone(),
            ]
        })
    );

    // Default: guard == NOT(OR(case1, case2)).
    let ed = req.queries.get("newbie_email").unwrap();
    assert_eq!(
        ed.when,
        Some(Filter::Not {
            filter: Box::new(Filter::Or {
                filters: vec![case1, case2]
            })
        })
    );
}

#[test]
fn switch_single_case_plus_default_generates_simple_complementary_guards() {
    let mut b = Batch::new();
    let case1 = filter::eq("active", true);

    let handles = b.switch(
        vec![(
            "when_active",
            case1.clone(),
            write::insert("log").row(doc().set("msg", "active")),
        )],
        (
            "when_inactive",
            write::insert("log").row(doc().set("msg", "inactive")),
        ),
    );
    assert_eq!(handles.len(), 2);

    let req = b.build();
    assert_eq!(
        req.queries.get("when_active").unwrap().when,
        Some(case1.clone())
    );
    assert_eq!(
        req.queries.get("when_inactive").unwrap().when,
        Some(Filter::Not {
            filter: Box::new(Filter::Or {
                filters: vec![case1]
            })
        })
    );
}

/// Gap #6 (Epic03/D brief item 6): `switch` with 4 cases + default —
/// confirms the NOT/OR chain accumulation is correct beyond the
/// already-covered 1-2-case shapes: each case's guard must AND-in a NOT
/// for every PRIOR case's condition (not just the immediately preceding
/// one), and the default's guard must NOT(OR(all 4 conditions)).
#[test]
fn switch_four_cases_plus_default_accumulates_full_not_or_chain() {
    let mut b = Batch::new();
    let case1 = filter::eq("tier", "vip");
    let case2 = filter::eq("tier", "regular");
    let case3 = filter::eq("tier", "trial");
    let case4 = filter::eq("tier", "guest");

    let handles = b.switch(
        vec![
            (
                "vip_email",
                case1.clone(),
                write::insert("emails").row(doc().set("kind", "vip")),
            ),
            (
                "regular_email",
                case2.clone(),
                write::insert("emails").row(doc().set("kind", "regular")),
            ),
            (
                "trial_email",
                case3.clone(),
                write::insert("emails").row(doc().set("kind", "trial")),
            ),
            (
                "guest_email",
                case4.clone(),
                write::insert("emails").row(doc().set("kind", "guest")),
            ),
        ],
        (
            "default_email",
            write::insert("emails").row(doc().set("kind", "default")),
        ),
    );

    assert_eq!(handles.len(), 5);

    let req = b.build();

    // Case 1: guard == case1 (no priors).
    assert_eq!(
        req.queries.get("vip_email").unwrap().when,
        Some(case1.clone())
    );

    // Case 2: guard == AND(NOT case1, case2).
    assert_eq!(
        req.queries.get("regular_email").unwrap().when,
        Some(Filter::And {
            filters: vec![
                Filter::Not {
                    filter: Box::new(case1.clone())
                },
                case2.clone(),
            ]
        })
    );

    // Case 3: guard == AND(NOT case2, AND(NOT case1, case3)) — folded
    // left-to-right over all PRIOR conditions (case1, case2).
    assert_eq!(
        req.queries.get("trial_email").unwrap().when,
        Some(Filter::And {
            filters: vec![
                Filter::Not {
                    filter: Box::new(case2.clone())
                },
                Filter::And {
                    filters: vec![
                        Filter::Not {
                            filter: Box::new(case1.clone())
                        },
                        case3.clone(),
                    ]
                },
            ]
        })
    );

    // Case 4: guard folds over ALL THREE priors (case1, case2, case3).
    assert_eq!(
        req.queries.get("guest_email").unwrap().when,
        Some(Filter::And {
            filters: vec![
                Filter::Not {
                    filter: Box::new(case3.clone())
                },
                Filter::And {
                    filters: vec![
                        Filter::Not {
                            filter: Box::new(case2.clone())
                        },
                        Filter::And {
                            filters: vec![
                                Filter::Not {
                                    filter: Box::new(case1.clone())
                                },
                                case4.clone(),
                            ]
                        },
                    ]
                },
            ]
        })
    );

    // Default: guard == NOT(OR(case1, case2, case3, case4)) — all four
    // conditions collected, not just the last one.
    let ed = req.queries.get("default_email").unwrap();
    assert_eq!(
        ed.when,
        Some(Filter::Not {
            filter: Box::new(Filter::Or {
                filters: vec![case1, case2, case3, case4]
            })
        })
    );
}

#[test]
fn switch_entries_participate_in_try_build_validation() {
    let mut b = Batch::new();
    let _mk = b.create_table("mk_tbl", ddl::create_table("emails").repo("main"));
    b.switch(
        vec![(
            "when_active",
            filter::eq("active", true),
            write::insert("emails").row(doc().set("kind", "active")),
        )],
        (
            "when_inactive",
            write::insert("emails").row(doc().set("kind", "inactive")),
        ),
    );

    assert!(b.try_build().is_ok());
}
