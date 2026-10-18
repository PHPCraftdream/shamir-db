use crate::expr::{ExprError, IndexExpr};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

fn intern(i: &Interner, s: &str) -> u64 {
    match i.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn make_rec(interner: &Interner) -> InnerValue {
    let mut m = new_map_wc(5);
    m.insert(
        InternerKey::new(intern(interner, "email")),
        InnerValue::Str("  Alice@Example.COM  ".into()),
    );
    m.insert(
        InternerKey::new(intern(interner, "age")),
        InnerValue::Int(30),
    );
    m.insert(
        InternerKey::new(intern(interner, "tags")),
        InnerValue::List(vec![
            InnerValue::Str("a".into()),
            InnerValue::Str("b".into()),
        ]),
    );
    m.insert(InternerKey::new(intern(interner, "name")), InnerValue::Null);
    InnerValue::Map(m)
}

#[test]
fn lower() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Lower(Box::new(IndexExpr::Field(vec![intern(&i, "email")])));
    let v = expr.eval(&rec).unwrap();
    assert_eq!(v, InnerValue::Str("  alice@example.com  ".into()));
}

#[test]
fn upper() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Upper(Box::new(IndexExpr::Field(vec![intern(&i, "email")])));
    let v = expr.eval(&rec).unwrap();
    assert_eq!(v, InnerValue::Str("  ALICE@EXAMPLE.COM  ".into()));
}

#[test]
fn trim() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Trim(Box::new(IndexExpr::Field(vec![intern(&i, "email")])));
    let v = expr.eval(&rec).unwrap();
    assert_eq!(v, InnerValue::Str("Alice@Example.COM".into()));
}

#[test]
fn lower_trim_compose() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Lower(Box::new(IndexExpr::Trim(Box::new(IndexExpr::Field(vec![
        intern(&i, "email"),
    ])))));
    let v = expr.eval(&rec).unwrap();
    assert_eq!(v, InnerValue::Str("alice@example.com".into()));
}

#[test]
fn length_string() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Length(Box::new(IndexExpr::Field(vec![intern(&i, "email")])));
    assert_eq!(expr.eval(&rec).unwrap(), InnerValue::Int(21));
}

#[test]
fn length_list() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Length(Box::new(IndexExpr::Field(vec![intern(&i, "tags")])));
    assert_eq!(expr.eval(&rec).unwrap(), InnerValue::Int(2));
}

#[test]
fn substring() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Substring {
        src: Box::new(IndexExpr::Trim(Box::new(IndexExpr::Field(vec![intern(
            &i, "email",
        )])))),
        start: 0,
        len: 5,
    };
    assert_eq!(expr.eval(&rec).unwrap(), InnerValue::Str("Alice".into()));
}

#[test]
fn modulo() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Mod(Box::new(IndexExpr::Field(vec![intern(&i, "age")])), 7);
    assert_eq!(expr.eval(&rec).unwrap(), InnerValue::Int(2)); // 30 % 7 = 2
}

#[test]
fn modulo_div_zero() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Mod(Box::new(IndexExpr::Field(vec![intern(&i, "age")])), 0);
    assert!(matches!(expr.eval(&rec), Err(ExprError::DivisionByZero)));
}

#[test]
fn coalesce_skips_null() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Coalesce(vec![
        IndexExpr::Field(vec![intern(&i, "name")]),        // Null
        IndexExpr::Field(vec![intern(&i, "nonexistent")]), // FieldNotFound
        IndexExpr::Field(vec![intern(&i, "email")]),       // actual
    ]);
    let v = expr.eval(&rec).unwrap();
    assert_eq!(v, InnerValue::Str("  Alice@Example.COM  ".into()));
}

#[test]
fn coalesce_all_null() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Coalesce(vec![
        IndexExpr::Field(vec![intern(&i, "name")]),
        IndexExpr::Field(vec![intern(&i, "nonexistent")]),
    ]);
    assert_eq!(expr.eval(&rec).unwrap(), InnerValue::Null);
}

#[test]
fn concat() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Concat(vec![
        IndexExpr::Trim(Box::new(IndexExpr::Field(vec![intern(&i, "email")]))),
        IndexExpr::Field(vec![intern(&i, "age")]),
    ]);
    assert_eq!(
        expr.eval(&rec).unwrap(),
        InnerValue::Str("Alice@Example.COM30".into())
    );
}

#[test]
fn type_mismatch_on_lower_int() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Lower(Box::new(IndexExpr::Field(vec![intern(&i, "age")])));
    assert!(matches!(
        expr.eval(&rec),
        Err(ExprError::TypeMismatch { .. })
    ));
}

#[test]
fn field_not_found() {
    let i = Interner::new();
    let rec = make_rec(&i);
    let expr = IndexExpr::Field(vec![intern(&i, "unknown")]);
    assert!(matches!(expr.eval(&rec), Err(ExprError::FieldNotFound)));
}

#[test]
fn serde_round_trip() {
    let expr = IndexExpr::Lower(Box::new(IndexExpr::Trim(Box::new(IndexExpr::Field(vec![
        42,
    ])))));
    let bytes = bincode::serialize(&expr).unwrap();
    let got: IndexExpr = bincode::deserialize(&bytes).unwrap();
    match got {
        IndexExpr::Lower(inner) => match *inner {
            IndexExpr::Trim(inner2) => match *inner2 {
                IndexExpr::Field(p) => assert_eq!(p, vec![42]),
                _ => panic!("wrong inner2"),
            },
            _ => panic!("wrong inner"),
        },
        _ => panic!("wrong outer"),
    }
}
