//! Pure expression AST for functional indexes.
//!
//! `IndexExpr` is a closed whitelist of deterministic, side-effect-free
//! transforms over `InnerValue`. No I/O, no time-dependency, no WASM.
//! Each function is O(1) or O(n) in the size of the input string.

use serde::{Deserialize, Serialize};
use shamir_types::types::value::InnerValue;

/// A pure, deterministic expression that transforms a record's field
/// value into a computed index key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexExpr {
    /// Extract a field by interned path.
    Field(Vec<u64>),
    /// Lowercase a string value.
    Lower(Box<IndexExpr>),
    /// Uppercase a string value.
    Upper(Box<IndexExpr>),
    /// Trim leading/trailing whitespace.
    Trim(Box<IndexExpr>),
    /// String or list length.
    Length(Box<IndexExpr>),
    /// Substring extraction.
    Substring {
        src: Box<IndexExpr>,
        start: u32,
        len: u32,
    },
    /// Traverse into a nested map by interned keys.
    JsonPath(Box<IndexExpr>, Vec<u64>),
    /// Concatenate N expressions' string results.
    Concat(Vec<IndexExpr>),
    /// Integer modulo (useful for shard/partition indexes).
    Mod(Box<IndexExpr>, i64),
    /// First non-null value (NULL handling).
    Coalesce(Vec<IndexExpr>),
}

#[derive(Debug, thiserror::Error)]
pub enum ExprError {
    #[error("type mismatch: expected {expected}, got {got}")]
    TypeMismatch { expected: &'static str, got: String },
    #[error("field not found")]
    FieldNotFound,
    #[error("division by zero")]
    DivisionByZero,
}

impl IndexExpr {
    /// Evaluate the expression against a record. The record must be
    /// `InnerValue::Map` at the top level (as stored by the engine).
    pub fn eval(&self, rec: &InnerValue) -> Result<InnerValue, ExprError> {
        match self {
            IndexExpr::Field(path) => resolve_path(rec, path),

            IndexExpr::Lower(inner) => match inner.eval(rec)? {
                InnerValue::Str(s) => Ok(InnerValue::Str(s.to_lowercase())),
                other => Err(type_err("string", &other)),
            },

            IndexExpr::Upper(inner) => match inner.eval(rec)? {
                InnerValue::Str(s) => Ok(InnerValue::Str(s.to_uppercase())),
                other => Err(type_err("string", &other)),
            },

            IndexExpr::Trim(inner) => match inner.eval(rec)? {
                InnerValue::Str(s) => Ok(InnerValue::Str(s.trim().to_string())),
                other => Err(type_err("string", &other)),
            },

            IndexExpr::Length(inner) => match inner.eval(rec)? {
                InnerValue::Str(s) => Ok(InnerValue::Int(s.len() as i64)),
                InnerValue::List(v) => Ok(InnerValue::Int(v.len() as i64)),
                other => Err(type_err("string or list", &other)),
            },

            IndexExpr::Substring { src, start, len } => match src.eval(rec)? {
                InnerValue::Str(s) => {
                    let start = *start as usize;
                    let len = *len as usize;
                    let chars: Vec<char> = s.chars().skip(start).take(len).collect();
                    Ok(InnerValue::Str(chars.into_iter().collect()))
                }
                other => Err(type_err("string", &other)),
            },

            IndexExpr::JsonPath(inner, segments) => {
                let val = inner.eval(rec)?;
                resolve_path(&val, segments)
            }

            IndexExpr::Concat(exprs) => {
                let mut out = String::new();
                for e in exprs {
                    match e.eval(rec)? {
                        InnerValue::Str(s) => out.push_str(&s),
                        InnerValue::Int(n) => out.push_str(&n.to_string()),
                        InnerValue::F64(f) => out.push_str(&f.to_string()),
                        InnerValue::Bool(b) => out.push_str(if b { "true" } else { "false" }),
                        InnerValue::Null => out.push_str("null"),
                        other => return Err(type_err("stringifiable", &other)),
                    }
                }
                Ok(InnerValue::Str(out))
            }

            IndexExpr::Mod(inner, divisor) => {
                if *divisor == 0 {
                    return Err(ExprError::DivisionByZero);
                }
                match inner.eval(rec)? {
                    InnerValue::Int(n) => Ok(InnerValue::Int(n % divisor)),
                    other => Err(type_err("int", &other)),
                }
            }

            IndexExpr::Coalesce(exprs) => {
                for e in exprs {
                    match e.eval(rec) {
                        Ok(InnerValue::Null) | Err(ExprError::FieldNotFound) => continue,
                        result => return result,
                    }
                }
                Ok(InnerValue::Null)
            }
        }
    }
}

fn resolve_path(val: &InnerValue, path: &[u64]) -> Result<InnerValue, ExprError> {
    let mut current = val;
    for &seg in path {
        match current {
            InnerValue::Map(m) => {
                let key = shamir_types::core::interner::InternerKey::new(seg);
                match m.get(&key) {
                    Some(v) => current = v,
                    None => return Err(ExprError::FieldNotFound),
                }
            }
            _ => return Err(ExprError::FieldNotFound),
        }
    }
    Ok(current.clone())
}

fn type_err(expected: &'static str, got: &InnerValue) -> ExprError {
    ExprError::TypeMismatch {
        expected,
        got: format!("{:?}", std::mem::discriminant(got)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_types::core::interner::{Interner, TouchInd};
    use shamir_types::types::common::new_map_wc;

    fn intern(i: &Interner, s: &str) -> u64 {
        match i.touch_ind(s).unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        }
    }

    fn make_rec(interner: &Interner) -> InnerValue {
        let mut m = new_map_wc(5);
        m.insert(
            shamir_types::core::interner::InternerKey::new(intern(interner, "email")),
            InnerValue::Str("  Alice@Example.COM  ".into()),
        );
        m.insert(
            shamir_types::core::interner::InternerKey::new(intern(interner, "age")),
            InnerValue::Int(30),
        );
        m.insert(
            shamir_types::core::interner::InternerKey::new(intern(interner, "tags")),
            InnerValue::List(vec![
                InnerValue::Str("a".into()),
                InnerValue::Str("b".into()),
            ]),
        );
        m.insert(
            shamir_types::core::interner::InternerKey::new(intern(interner, "name")),
            InnerValue::Null,
        );
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
        let expr = IndexExpr::Lower(Box::new(IndexExpr::Trim(Box::new(IndexExpr::Field(
            vec![intern(&i, "email")],
        )))));
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
            IndexExpr::Field(vec![intern(&i, "name")]),       // Null
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
        let expr = IndexExpr::Lower(Box::new(IndexExpr::Trim(Box::new(IndexExpr::Field(
            vec![42],
        )))));
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
}
