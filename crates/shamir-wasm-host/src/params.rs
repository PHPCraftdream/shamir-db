//! Typed accessor over a function's call parameters.
//!
//! A function's inputs arrive as a string-keyed map of [`QueryValue`]s. The
//! call site decides what fills it: explicit `args` for a batch-node call,
//! the current row for a `where`/`set` expression, the inserted record for
//! key generation. [`Params`] is the uniform read surface either way.

use super::error::{FnResult, FunctionError};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;

/// Call parameters: a string-keyed map of [`QueryValue`]s.
#[derive(Debug, Clone, Default)]
pub struct Params {
    map: TMap<String, QueryValue>,
}

impl Params {
    /// An empty parameter set.
    pub fn new() -> Self {
        Self { map: new_map() }
    }

    /// Wrap an existing map.
    pub fn from_map(map: TMap<String, QueryValue>) -> Self {
        Self { map }
    }

    /// Build from a `QueryValue::Map`; errors if `value` is not a map.
    pub fn from_value(value: QueryValue) -> FnResult<Self> {
        match value {
            QueryValue::Map(map) => Ok(Self { map }),
            _ => Err(FunctionError::BadParam {
                name: "<params>".into(),
                reason: "expected an object/map".into(),
            }),
        }
    }

    /// Insert/overwrite a parameter (builder-style).
    pub fn set(&mut self, key: impl Into<String>, value: QueryValue) -> &mut Self {
        self.map.insert(key.into(), value);
        self
    }

    /// The raw underlying map.
    pub fn raw(&self) -> &TMap<String, QueryValue> {
        &self.map
    }

    /// Required parameter by key.
    pub fn get(&self, key: &str) -> FnResult<&QueryValue> {
        self.map
            .get(key)
            .ok_or_else(|| FunctionError::MissingParam(key.to_string()))
    }

    /// Required byte string (`Bin`, or `Str` decoded as UTF-8 bytes).
    pub fn bytes(&self, key: &str) -> FnResult<Vec<u8>> {
        match self.get(key)? {
            QueryValue::Bin(b) => Ok(b.clone()),
            QueryValue::Str(s) => Ok(s.as_bytes().to_vec()),
            _ => Err(Self::bad(key, "expected binary or string")),
        }
    }

    /// Required string.
    pub fn str(&self, key: &str) -> FnResult<&str> {
        match self.get(key)? {
            QueryValue::Str(s) => Ok(s.as_str()),
            _ => Err(Self::bad(key, "expected string")),
        }
    }

    /// Required `u32` (an `Int` in `0..=u32::MAX`).
    pub fn u32(&self, key: &str) -> FnResult<u32> {
        match self.get(key)? {
            QueryValue::Int(i) if *i >= 0 && *i <= i64::from(u32::MAX) => Ok(*i as u32),
            QueryValue::Int(_) => Err(Self::bad(key, "out of u32 range")),
            _ => Err(Self::bad(key, "expected integer")),
        }
    }

    /// Optional `u32`: `None` when absent or explicitly `Null`.
    pub fn opt_u32(&self, key: &str) -> FnResult<Option<u32>> {
        match self.map.get(key) {
            None | Some(QueryValue::Null) => Ok(None),
            Some(_) => self.u32(key).map(Some),
        }
    }

    fn bad(name: &str, reason: &str) -> FunctionError {
        FunctionError::BadParam {
            name: name.into(),
            reason: reason.into(),
        }
    }
}
