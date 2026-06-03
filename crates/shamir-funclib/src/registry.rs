//! The scalar-function contract every category module builds against.
//!
//! A scalar function is a pure `fn(&[InnerValue]) -> ScalarResult`. The
//! [`ScalarRegistry`] owns a name → [`FnEntry`] table, validates arity on
//! dispatch, and exposes the value-extraction/-construction helpers that all
//! category modules share so argument handling stays uniform.
//!
//! Errors are **machine-readable codes only** — no human text. The frontend
//! localises by code (e.g. `"type_mismatch"`, `"arity"`, `"unknown_function"`).

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use shamir_types::types::value::InnerValue;
use std::collections::HashMap;
use std::sync::Arc;

/// Machine-readable scalar-error. Carries a stable `code`; the frontend maps the
/// code to a localised human message. No human-facing text lives here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarError {
    pub code: String,
}

impl ScalarError {
    /// Construct an error from a stable machine code (e.g. `"type_mismatch"`).
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

impl std::fmt::Display for ScalarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code)
    }
}

impl std::error::Error for ScalarError {}

/// The result every scalar function returns.
pub type ScalarResult = Result<InnerValue, ScalarError>;

/// A scalar function: pure, `Send + Sync`, dispatched by name.
pub type ScalarFn = Arc<dyn Fn(&[InnerValue]) -> ScalarResult + Send + Sync>;

/// Registry entry: the callable plus its arity bounds and purity metadata.
///
/// `min_args` / `max_args` bound the argument count (`max_args = None` ⇒
/// variadic / unbounded, used by n-ary `min`/`max`). `pure` and `deterministic`
/// drive indexability: only a `pure + deterministic` function may back a
/// functional index.
#[derive(Clone)]
pub struct FnEntry {
    pub f: ScalarFn,
    pub min_args: usize,
    pub max_args: Option<usize>,
    pub pure: bool,
    pub deterministic: bool,
}

impl FnEntry {
    /// Convenience constructor for the common case: a pure + deterministic fn
    /// with the given inclusive arity bounds.
    pub fn pure(
        f: impl Fn(&[InnerValue]) -> ScalarResult + Send + Sync + 'static,
        min_args: usize,
        max_args: Option<usize>,
    ) -> Self {
        Self {
            f: Arc::new(f),
            min_args,
            max_args,
            pure: true,
            deterministic: true,
        }
    }
}

/// Name → [`FnEntry`] table. When functions are registered via
/// [`in_folder`](Self::in_folder), names are folder-qualified (`"math/abs"`,
/// `"json/keys"`); a direct [`register`](Self::register) with no active folder
/// inserts the plain name as-is.
#[derive(Default)]
pub struct ScalarRegistry {
    fns: HashMap<String, FnEntry>,
    prefix: String,
}

impl ScalarRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            fns: HashMap::new(),
            prefix: String::new(),
        }
    }

    /// Register a function under `name`. When called inside an
    /// [`in_folder`](Self::in_folder) scope the name is automatically
    /// folder-qualified (`"math/abs"`); otherwise it is stored as-is.
    pub fn register(&mut self, name: impl Into<String>, entry: FnEntry) {
        let key = if self.prefix.is_empty() {
            name.into()
        } else {
            format!("{}/{}", self.prefix, name.into())
        };
        self.fns.insert(key, entry);
    }

    /// Register everything `f` registers under a `folder/` prefix, so
    /// categories that share a plain name (json/keys vs object/keys,
    /// math/min vs arrays/min) don't collide. Aligns with the
    /// function-folder catalogue (#118).
    pub fn in_folder<F: FnOnce(&mut Self)>(&mut self, folder: &str, f: F) {
        let prev = std::mem::replace(&mut self.prefix, folder.to_string());
        f(self);
        self.prefix = prev;
    }

    /// Look up an entry by name.
    pub fn get(&self, name: &str) -> Option<&FnEntry> {
        self.fns.get(name)
    }

    /// Dispatch `name` with `args`, validating arity first.
    ///
    /// Returns `ScalarError("unknown_function")` if the name is not registered,
    /// or `ScalarError("arity")` if the argument count is outside the entry's
    /// `[min_args, max_args]` bounds.
    pub fn call(&self, name: &str, args: &[InnerValue]) -> ScalarResult {
        let entry = self
            .fns
            .get(name)
            .ok_or_else(|| ScalarError::new("unknown_function"))?;
        if args.len() < entry.min_args {
            return Err(ScalarError::new("arity"));
        }
        if let Some(max) = entry.max_args {
            if args.len() > max {
                return Err(ScalarError::new("arity"));
            }
        }
        (entry.f)(args)
    }

    /// All registered function names (unordered).
    pub fn names(&self) -> Vec<&str> {
        self.fns.keys().map(String::as_str).collect()
    }

    /// Number of registered functions.
    pub fn len(&self) -> usize {
        self.fns.len()
    }

    /// Whether the registry holds no functions.
    pub fn is_empty(&self) -> bool {
        self.fns.is_empty()
    }
}

// ===========================================================================
// Argument extractors — shared by every category module.
//
// All take `(args, i)` and validate both presence ("missing_arg") and type
// ("type_mismatch") / range ("out_of_range"). Numeric extractors coerce across
// the numeric InnerValue variants (Int / Dec / F64 / Bool-as-0/1 where noted)
// so a category fn does not re-implement coercion.
// ===========================================================================

/// Fetch the `i`-th argument or `ScalarError("missing_arg")`.
fn at(args: &[InnerValue], i: usize) -> Result<&InnerValue, ScalarError> {
    args.get(i).ok_or_else(|| ScalarError::new("missing_arg"))
}

/// Extract an `i64`. Accepts `Int`, `Bool` (0/1), and integral `Dec`/`F64`
/// (rejects fractional or out-of-range values with `"out_of_range"`).
pub fn arg_i64(args: &[InnerValue], i: usize) -> Result<i64, ScalarError> {
    match at(args, i)? {
        InnerValue::Int(n) => Ok(*n),
        InnerValue::Bool(b) => Ok(*b as i64),
        InnerValue::Dec(d) => {
            if d.fract().is_zero() {
                d.to_i64().ok_or_else(|| ScalarError::new("out_of_range"))
            } else {
                Err(ScalarError::new("out_of_range"))
            }
        }
        InnerValue::F64(f) => {
            if f.fract() == 0.0 && f.is_finite() && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 {
                Ok(*f as i64)
            } else {
                Err(ScalarError::new("out_of_range"))
            }
        }
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Extract a `&str` from a `Str` argument.
pub fn arg_str(args: &[InnerValue], i: usize) -> Result<&str, ScalarError> {
    match at(args, i)? {
        InnerValue::Str(s) => Ok(s.as_str()),
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Extract an `f64`. Accepts `F64`, `Int`, `Dec` (lossy), and `Bool` (0/1).
pub fn arg_f64(args: &[InnerValue], i: usize) -> Result<f64, ScalarError> {
    match at(args, i)? {
        InnerValue::F64(f) => Ok(*f),
        InnerValue::Int(n) => Ok(*n as f64),
        InnerValue::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        InnerValue::Dec(d) => d.to_f64().ok_or_else(|| ScalarError::new("out_of_range")),
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Extract a [`Decimal`]. Accepts `Dec`, `Int`, `Bool` (0/1), and finite `F64`.
pub fn arg_dec(args: &[InnerValue], i: usize) -> Result<Decimal, ScalarError> {
    match at(args, i)? {
        InnerValue::Dec(d) => Ok(*d),
        InnerValue::Int(n) => Ok(Decimal::from(*n)),
        InnerValue::Bool(b) => Ok(Decimal::from(*b as i64)),
        InnerValue::F64(f) => {
            Decimal::from_f64_retain(*f).ok_or_else(|| ScalarError::new("out_of_range"))
        }
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Extract a `bool` from a `Bool` argument.
pub fn arg_bool(args: &[InnerValue], i: usize) -> Result<bool, ScalarError> {
    match at(args, i)? {
        InnerValue::Bool(b) => Ok(*b),
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Extract a `&[InnerValue]` from a `List` argument.
pub fn arg_list(args: &[InnerValue], i: usize) -> Result<&[InnerValue], ScalarError> {
    match at(args, i)? {
        InnerValue::List(l) => Ok(l.as_slice()),
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Extract a `&[u8]` from a `Bin` argument.
pub fn arg_bytes(args: &[InnerValue], i: usize) -> Result<&[u8], ScalarError> {
    match at(args, i)? {
        InnerValue::Bin(b) => Ok(b.as_slice()),
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

// ===========================================================================
// Value constructors — shared by every category module.
// ===========================================================================

/// Construct an `Int`.
pub fn v_int(n: i64) -> InnerValue {
    InnerValue::Int(n)
}

/// Construct a `Str`.
pub fn v_str(s: String) -> InnerValue {
    InnerValue::Str(s)
}

/// Construct a `Bool`.
pub fn v_bool(b: bool) -> InnerValue {
    InnerValue::Bool(b)
}

/// Construct a numeric value from an `f64`, stored as `Dec` to keep the value
/// model decimal-first (per the design doc: "decimal math uses `Dec`"). A
/// non-finite `f64` (NaN/±∞) yields `ScalarError("out_of_range")`.
pub fn v_f64(f: f64) -> ScalarResult {
    Decimal::from_f64_retain(f)
        .map(InnerValue::Dec)
        .ok_or_else(|| ScalarError::new("out_of_range"))
}

/// Construct a `Dec`.
pub fn v_dec(d: Decimal) -> InnerValue {
    InnerValue::Dec(d)
}

/// Construct a `List`.
pub fn v_list(items: Vec<InnerValue>) -> InnerValue {
    InnerValue::List(items)
}

/// Construct a `Bin`.
pub fn v_bytes(b: Vec<u8>) -> InnerValue {
    InnerValue::Bin(b)
}
