//! Unit tests for `FieldRule::check` — type assertions and constraint checks.
//!
//! All tests use [`OwnedFields`] (backed by `QueryValue::Map`) because it is
//! the simplest to construct and covers the INSERT/UPDATE path.

use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::value::QueryValue;

use crate::validator::encode::Validation;
use crate::validator::record_fields::OwnedFields;
use crate::validator::schema::constraints::{Constraints, Num};
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::type_tag::TypeTag;

// ── helpers ─────────────────────────────────────────────────────────────────

fn fields_from(pairs: Vec<(&str, QueryValue)>) -> QueryValue {
    let mut m = new_map();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    QueryValue::Map(m)
}

fn check_rule(rule: &FieldRule, qv: &QueryValue) -> Validation {
    let fields = OwnedFields { qv };
    let path_refs: Vec<&str> = rule.path.iter().map(String::as_str).collect();
    let mut v = Validation::accept();
    rule.check(&fields, &path_refs, &mut v);
    v
}

fn simple_rule(path: &str, ty: TypeTag) -> FieldRule {
    FieldRule {
        path: vec![path.to_string()],
        ty,
        constraints: Constraints::default(),
    }
}

// ── Type tag checks ─────────────────────────────────────────────────────────

#[test]
fn string_accepts_str() {
    let rule = simple_rule("name", TypeTag::String);
    let qv = fields_from(vec![("name", QueryValue::Str("alice".into()))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn string_rejects_int() {
    let rule = simple_rule("name", TypeTag::String);
    let qv = fields_from(vec![("name", QueryValue::Int(42))]);
    let v = check_rule(&rule, &qv);
    assert!(!v.is_ok());
    assert_eq!(v.errors[0].code, "type_mismatch");
}

#[test]
fn int_accepts_int() {
    let rule = simple_rule("age", TypeTag::Int);
    let qv = fields_from(vec![("age", QueryValue::Int(25))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn int_rejects_str() {
    let rule = simple_rule("age", TypeTag::Int);
    let qv = fields_from(vec![("age", QueryValue::Str("twenty".into()))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "type_mismatch");
}

#[test]
fn f64_accepts_f64() {
    let rule = simple_rule("score", TypeTag::F64);
    let qv = fields_from(vec![("score", QueryValue::F64(2.72))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn f64_rejects_int() {
    let rule = simple_rule("score", TypeTag::F64);
    let qv = fields_from(vec![("score", QueryValue::Int(3))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "type_mismatch");
}

#[test]
fn bool_accepts_bool() {
    let rule = simple_rule("active", TypeTag::Bool);
    let qv = fields_from(vec![("active", QueryValue::Bool(true))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn bool_rejects_str() {
    let rule = simple_rule("active", TypeTag::Bool);
    let qv = fields_from(vec![("active", QueryValue::Str("true".into()))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "type_mismatch");
}

#[test]
fn bin_accepts_bin() {
    let rule = simple_rule("data", TypeTag::Bin);
    let qv = fields_from(vec![("data", QueryValue::Bin(vec![0xDE, 0xAD]))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn list_accepts_list() {
    let rule = simple_rule("tags", TypeTag::List);
    let qv = fields_from(vec![(
        "tags",
        QueryValue::List(vec![QueryValue::Str("a".into())]),
    )]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn list_rejects_map() {
    let rule = simple_rule("tags", TypeTag::List);
    let qv = fields_from(vec![("tags", QueryValue::Map(new_map()))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "type_mismatch");
}

#[test]
fn map_accepts_map() {
    let rule = simple_rule("meta", TypeTag::Map);
    let qv = fields_from(vec![("meta", QueryValue::Map(new_map()))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn set_accepts_set() {
    let rule = simple_rule("ids", TypeTag::Set);
    let mut s = new_set();
    s.insert(QueryValue::Int(1));
    let qv = fields_from(vec![("ids", QueryValue::Set(s))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn dec_accepts_dec_on_owned_fields() {
    let rule = simple_rule("price", TypeTag::Dec);
    let qv = fields_from(vec![(
        "price",
        QueryValue::Dec(rust_decimal::Decimal::new(1999, 2)),
    )]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn any_accepts_anything() {
    let rule = simple_rule("x", TypeTag::Any);
    let qv = fields_from(vec![("x", QueryValue::Int(42))]);
    assert!(check_rule(&rule, &qv).is_ok());

    let qv2 = fields_from(vec![("x", QueryValue::Str("hello".into()))]);
    assert!(check_rule(&rule, &qv2).is_ok());
}

// ── Numeric constraints ─────────────────────────────────────────────────────

#[test]
fn int_min_max_accept() {
    let rule = FieldRule {
        path: vec!["age".into()],
        ty: TypeTag::Int,
        constraints: Constraints {
            min: Some(Num::Int(0)),
            max: Some(Num::Int(150)),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("age", QueryValue::Int(25))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn int_min_reject() {
    let rule = FieldRule {
        path: vec!["age".into()],
        ty: TypeTag::Int,
        constraints: Constraints {
            min: Some(Num::Int(0)),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("age", QueryValue::Int(-1))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "out_of_range");
}

#[test]
fn int_max_reject() {
    let rule = FieldRule {
        path: vec!["age".into()],
        ty: TypeTag::Int,
        constraints: Constraints {
            max: Some(Num::Int(150)),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("age", QueryValue::Int(200))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "out_of_range");
}

#[test]
fn int_unsigned_reject_negative() {
    let rule = FieldRule {
        path: vec!["count".into()],
        ty: TypeTag::Int,
        constraints: Constraints {
            unsigned: true,
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("count", QueryValue::Int(-5))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "out_of_range");
}

#[test]
fn int_unsigned_accept_zero() {
    let rule = FieldRule {
        path: vec!["count".into()],
        ty: TypeTag::Int,
        constraints: Constraints {
            unsigned: true,
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("count", QueryValue::Int(0))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn f64_min_max_accept() {
    let rule = FieldRule {
        path: vec!["score".into()],
        ty: TypeTag::F64,
        constraints: Constraints {
            min: Some(Num::F64(0.0)),
            max: Some(Num::F64(100.0)),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("score", QueryValue::F64(50.5))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn f64_out_of_range() {
    let rule = FieldRule {
        path: vec!["score".into()],
        ty: TypeTag::F64,
        constraints: Constraints {
            max: Some(Num::F64(100.0)),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("score", QueryValue::F64(100.1))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "out_of_range");
}

// ── String constraints ──────────────────────────────────────────────────────

#[test]
fn string_exact_len_accept() {
    let rule = FieldRule {
        path: vec!["zip".into()],
        ty: TypeTag::String,
        constraints: Constraints {
            len: Some(5),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("zip", QueryValue::Str("12345".into()))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn string_exact_len_reject() {
    let rule = FieldRule {
        path: vec!["zip".into()],
        ty: TypeTag::String,
        constraints: Constraints {
            len: Some(5),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("zip", QueryValue::Str("1234".into()))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "wrong_length");
}

#[test]
fn string_max_len_accept() {
    let rule = FieldRule {
        path: vec!["name".into()],
        ty: TypeTag::String,
        constraints: Constraints {
            max_len: Some(10),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("name", QueryValue::Str("alice".into()))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn string_max_len_reject() {
    let rule = FieldRule {
        path: vec!["name".into()],
        ty: TypeTag::String,
        constraints: Constraints {
            max_len: Some(3),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("name", QueryValue::Str("alice".into()))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "too_long");
}

#[test]
fn string_min_len_reject() {
    let rule = FieldRule {
        path: vec!["name".into()],
        ty: TypeTag::String,
        constraints: Constraints {
            min_len: Some(5),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("name", QueryValue::Str("ab".into()))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "too_short");
}

// ── Collection constraints ──────────────────────────────────────────────────

#[test]
fn list_exact_len_accept() {
    let rule = FieldRule {
        path: vec!["items".into()],
        ty: TypeTag::List,
        constraints: Constraints {
            len: Some(2),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![(
        "items",
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)]),
    )]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn list_exact_len_reject() {
    let rule = FieldRule {
        path: vec!["items".into()],
        ty: TypeTag::List,
        constraints: Constraints {
            len: Some(2),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("items", QueryValue::List(vec![QueryValue::Int(1)]))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "wrong_length");
}

#[test]
fn list_max_len_reject() {
    let rule = FieldRule {
        path: vec!["items".into()],
        ty: TypeTag::List,
        constraints: Constraints {
            max_len: Some(1),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![(
        "items",
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)]),
    )]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "too_long");
}

#[test]
fn list_min_len_reject() {
    let rule = FieldRule {
        path: vec!["items".into()],
        ty: TypeTag::List,
        constraints: Constraints {
            min_len: Some(3),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("items", QueryValue::List(vec![QueryValue::Int(1)]))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "too_short");
}

// ── array_of ────────────────────────────────────────────────────────────────

#[test]
fn array_of_string_accept() {
    let rule = FieldRule {
        path: vec!["tags".into()],
        ty: TypeTag::List,
        constraints: Constraints {
            array_of: Some(TypeTag::String),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![(
        "tags",
        QueryValue::List(vec![
            QueryValue::Str("a".into()),
            QueryValue::Str("b".into()),
        ]),
    )]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn array_of_string_reject_mixed() {
    let rule = FieldRule {
        path: vec!["tags".into()],
        ty: TypeTag::List,
        constraints: Constraints {
            array_of: Some(TypeTag::String),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![(
        "tags",
        QueryValue::List(vec![QueryValue::Str("a".into()), QueryValue::Int(42)]),
    )]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "type_mismatch");
}

#[test]
fn array_of_int_accept_empty() {
    let rule = FieldRule {
        path: vec!["ids".into()],
        ty: TypeTag::List,
        constraints: Constraints {
            array_of: Some(TypeTag::Int),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("ids", QueryValue::List(vec![]))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

// ── one_of ──────────────────────────────────────────────────────────────────

#[test]
fn one_of_accept() {
    let rule = FieldRule {
        path: vec!["status".into()],
        ty: TypeTag::String,
        constraints: Constraints {
            one_of: Some(vec![
                QueryValue::Str("active".into()),
                QueryValue::Str("inactive".into()),
            ]),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("status", QueryValue::Str("active".into()))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn one_of_reject() {
    let rule = FieldRule {
        path: vec!["status".into()],
        ty: TypeTag::String,
        constraints: Constraints {
            one_of: Some(vec![
                QueryValue::Str("active".into()),
                QueryValue::Str("inactive".into()),
            ]),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("status", QueryValue::Str("deleted".into()))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "not_in_enum");
}

#[test]
fn one_of_int_accept() {
    let rule = FieldRule {
        path: vec!["priority".into()],
        ty: TypeTag::Int,
        constraints: Constraints {
            one_of: Some(vec![
                QueryValue::Int(1),
                QueryValue::Int(2),
                QueryValue::Int(3),
            ]),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("priority", QueryValue::Int(2))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn one_of_const_single_element() {
    let rule = FieldRule {
        path: vec!["version".into()],
        ty: TypeTag::Int,
        constraints: Constraints {
            one_of: Some(vec![QueryValue::Int(1)]),
            ..Default::default()
        },
    };
    let qv = fields_from(vec![("version", QueryValue::Int(1))]);
    assert!(check_rule(&rule, &qv).is_ok());

    let qv2 = fields_from(vec![("version", QueryValue::Int(2))]);
    let v = check_rule(&rule, &qv2);
    assert_eq!(v.errors[0].code, "not_in_enum");
}

// ── Nested path ─────────────────────────────────────────────────────────────

#[test]
fn nested_path_accepts() {
    let rule = FieldRule {
        path: vec!["address".into(), "zip".into()],
        ty: TypeTag::String,
        constraints: Constraints {
            len: Some(5),
            ..Default::default()
        },
    };

    let mut addr = new_map();
    addr.insert("zip".to_string(), QueryValue::Str("12345".into()));
    let qv = fields_from(vec![("address", QueryValue::Map(addr))]);
    assert!(check_rule(&rule, &qv).is_ok());
}

#[test]
fn nested_path_wrong_type() {
    let rule = FieldRule {
        path: vec!["address".into(), "zip".into()],
        ty: TypeTag::String,
        constraints: Constraints::default(),
    };

    let mut addr = new_map();
    addr.insert("zip".to_string(), QueryValue::Int(12345));
    let qv = fields_from(vec![("address", QueryValue::Map(addr))]);
    let v = check_rule(&rule, &qv);
    assert_eq!(v.errors[0].code, "type_mismatch");
}
