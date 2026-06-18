//! Pure expression AST for functional indexes.
//!
//! `IndexExpr` is a closed whitelist of deterministic, side-effect-free
//! transforms over `InnerValue`. No I/O, no time-dependency, no WASM.
//! Each function is O(1) or O(n) in the size of the input string.
//!
//! §5b floor: `eval` takes a `RecordRef` lens (no input conversion); its
//! result is an OWNED COMPUTED value (a transform output, not a record
//! materialization), so it is irreducibly `InnerValue`. The single record
//! touch is `materialize_at` on the `Field` leaf — the `RecordRef` trait's
//! documented escape hatch.

use serde::{Deserialize, Serialize};
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordRef;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;

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
    NestedPath(Box<IndexExpr>, Vec<u64>),
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
    /// a map at the top level (as stored by the engine).
    ///
    /// Generic over `RecordRef` so both `InnerValue` (tree) and
    /// `RecordView` (zero-copy lens) callers work without conversion.
    /// Returns an owned computed value (§5b floor — not a record materialization).
    pub fn eval(&self, rec: &(impl RecordRef + ?Sized)) -> Result<InnerValue, ExprError> {
        match self {
            IndexExpr::Field(path) => {
                let ipath: SmallVec<[InternerKey; 4]> =
                    path.iter().map(|&id| InternerKey::new(id)).collect();
                rec.materialize_at(&ipath).ok_or(ExprError::FieldNotFound)
            }

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

            IndexExpr::NestedPath(inner, segments) => {
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
                let key = InternerKey::new(seg);
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
