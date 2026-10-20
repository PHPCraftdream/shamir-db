//! Guest-side typed parameter accessors.

use crate::error::{Error, Result};
use crate::Value;

/// Call parameters: a string-keyed map of [`Value`]s.
///
/// Decoded from the msgpack bytes the host sends through `shamir_call`.
#[derive(Debug, Clone, Default)]
pub struct Params {
    map: Vec<(String, Value)>,
}

impl Params {
    /// Empty parameter set.
    pub fn new() -> Self {
        Self { map: Vec::new() }
    }

    /// Wrap a pre-built map.
    pub fn from_map(map: Vec<(String, Value)>) -> Self {
        Self { map }
    }

    /// Generic typed getter.
    pub fn get(&self, key: &str) -> Result<&Value> {
        self.map
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .ok_or_else(|| Error::user(format!("missing parameter: {key}")))
    }

    /// Required `i64` (from `Value::Int`).
    pub fn i64(&self, key: &str) -> Result<i64> {
        match self.get(key)? {
            Value::Int(n) => Ok(*n),
            other => Err(Error::user(format!(
                "parameter `{key}`: expected integer, got {}",
                other.type_name()
            ))),
        }
    }

    /// Required `f64` (from `Value::F64`).
    pub fn f64(&self, key: &str) -> Result<f64> {
        match self.get(key)? {
            Value::F64(n) => Ok(*n),
            other => Err(Error::user(format!(
                "parameter `{key}`: expected float, got {}",
                other.type_name()
            ))),
        }
    }

    /// Required `&str` (from `Value::Str`).
    pub fn str(&self, key: &str) -> Result<&str> {
        match self.get(key)? {
            Value::Str(s) => Ok(s),
            other => Err(Error::user(format!(
                "parameter `{key}`: expected string, got {}",
                other.type_name()
            ))),
        }
    }

    /// Required bytes (from `Value::Bin`, or `Value::Str` as UTF-8 bytes).
    pub fn bytes(&self, key: &str) -> Result<Vec<u8>> {
        match self.get(key)? {
            Value::Bin(b) => Ok(b.clone()),
            Value::Str(s) => Ok(s.as_bytes().to_vec()),
            other => Err(Error::user(format!(
                "parameter `{key}`: expected binary or string, got {}",
                other.type_name()
            ))),
        }
    }

    /// Required `bool` (from `Value::Bool`).
    pub fn bool(&self, key: &str) -> Result<bool> {
        match self.get(key)? {
            Value::Bool(b) => Ok(*b),
            other => Err(Error::user(format!(
                "parameter `{key}`: expected bool, got {}",
                other.type_name()
            ))),
        }
    }

    /// The raw underlying map.
    pub fn raw(&self) -> &[(String, Value)] {
        &self.map
    }
}

impl Value {
    fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::F64(_) => "float",
            Value::Str(_) => "string",
            Value::Bin(_) => "binary",
            Value::List(_) => "list",
            Value::Map(_) => "map",
        }
    }
}
