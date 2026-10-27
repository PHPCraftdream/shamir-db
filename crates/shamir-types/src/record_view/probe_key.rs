//! Conservative owned candidate keys for staged scalar probes.
//!
//! Probe equality is not transitive: a stored string may match a numeric
//! literal after parsing, while two strings matching different numeric
//! literals need not match each other. Keys therefore narrow candidates only;
//! callers must always recheck with `scalar_ref_numeric_eq`.
use crate::types::numeric_key::{decimal_exact_parts, NumericKey};
use crate::types::value::QueryValue;
use num_bigint::BigInt;
use rust_decimal::Decimal;

use super::ScalarRef;

/// Candidate keys. A numeric string contributes every exact interpretation
/// accepted by the predicate, plus its literal string key. Keys only narrow
/// candidates; callers must always recheck with `scalar_ref_numeric_eq`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProbeKey {
    Null,
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    Numeric(NumericKey),
    Decimal(i128, u32),
}

impl ProbeKey {
    /// Candidate keys for a stored scalar: its exact type/value key, plus
    /// parsed numeric projections for strings.
    pub fn for_scalar(value: ScalarRef<'_>) -> Vec<Self> {
        match value {
            ScalarRef::Null => vec![Self::Null],
            ScalarRef::Bool(value) => vec![Self::Bool(value)],
            ScalarRef::Int(value) => vec![Self::Numeric(NumericKey::Int(value))],
            ScalarRef::F64(value) if value.is_nan() => Vec::new(),
            ScalarRef::F64(value) => vec![Self::Numeric(NumericKey::from_f64(value))],
            ScalarRef::Str(value) => string_keys(value),
            ScalarRef::Bin(value) => vec![Self::Bytes(value.to_vec())],
        }
    }

    /// Candidate keys for a probe literal. Numeric keys are canonical and
    /// string probes retain exact spelling; stored strings add parsed numeric
    /// projections to cover the predicate's cross-type cases.
    pub fn for_query(value: &QueryValue) -> Vec<Self> {
        match value {
            QueryValue::Null => vec![Self::Null],
            QueryValue::Bool(value) => vec![Self::Bool(*value)],
            QueryValue::Str(value) => vec![Self::String(value.clone())],
            QueryValue::Bin(value) => vec![Self::Bytes(value.clone())],
            QueryValue::Int(value) => vec![Self::Numeric(NumericKey::Int(*value))],
            QueryValue::F64(value) if value.is_nan() => Vec::new(),
            QueryValue::F64(value) => vec![Self::Numeric(NumericKey::from_f64(*value))],
            QueryValue::Big(value) => vec![Self::Numeric(NumericKey::from_bigint(value))],
            QueryValue::Dec(value) => decimal_keys(value),
            _ => Vec::new(),
        }
    }
}

fn decimal_keys(value: &Decimal) -> Vec<ProbeKey> {
    match NumericKey::from_decimal(value) {
        Some(key) => vec![ProbeKey::Numeric(key)],
        None => {
            let (mantissa, scale) = decimal_exact_parts(value);
            vec![ProbeKey::Decimal(mantissa, scale)]
        }
    }
}

fn string_keys(value: &str) -> Vec<ProbeKey> {
    let mut keys = vec![ProbeKey::String(value.to_owned())];
    if let Ok(decimal) = Decimal::from_str_exact(value) {
        keys.extend(decimal_keys(&decimal));
    }
    if let Some(big) = BigInt::parse_bytes(value.as_bytes(), 10) {
        keys.push(ProbeKey::Numeric(NumericKey::from_bigint(&big)));
    }
    keys
}
