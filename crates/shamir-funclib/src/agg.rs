//! Aggregate function registry — functions over a *stream* of rows
//! (N values -> 1 result).
//!
//! This module is a peer to [`crate::registry::ScalarRegistry`] but for
//! aggregates. Each aggregate is a stateful [`Aggregator`] created by a
//! factory; the engine calls [`Aggregator::accumulate`] for every row,
//! then [`Aggregator::finalize`] to produce the single result.
//!
//! ## Parameterised aggregators
//!
//! `percentile` and `string_agg` accept parameters. Their factories
//! capture the parameter at creation time:
//! - `percentile`: default registration uses `p = 0.5` (i.e. median).
//!   Use [`percentile`] constructor to supply a custom `p`.
//! - `string_agg`: default registration uses `sep = ","`.
//!   Use [`string_agg`] constructor to supply a custom separator.
//!
//! ## Empty-input convention
//!
//! - `count` / `count_distinct` -> `Int(0)`
//! - `sum` / `range` -> `Int(0)` (neutral element)
//! - `array_agg` -> empty `List`
//! - `bool_and` -> `Bool(true)` (identity for AND)
//! - `bool_or` -> `Bool(false)` (identity for OR)
//! - `avg` / `stddev` / `variance` -> `ScalarError("empty")`
//! - `min` / `max` / `first` / `last` / `median` / `mode` ->
//!   `ScalarError("empty")`
//! - `string_agg` -> `Str("")`
//! - `percentile` -> `ScalarError("empty")`

use crate::compare;
use crate::registry::ScalarError;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use shamir_collections::TFxMap;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Trait + registry
// ---------------------------------------------------------------------------

/// A stateful aggregate that consumes a stream of values and produces
/// a single result.
pub trait Aggregator: Send {
    /// Feed one value. `Null` values are silently skipped by most
    /// aggregators (SQL semantics); those that want Nulls override
    /// this default.
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError>;

    /// Consume the accumulator and return the final aggregate value.
    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError>;
}

/// Factory that produces a fresh [`Aggregator`] instance.
pub type AggFactory = Arc<dyn Fn() -> Box<dyn Aggregator> + Send + Sync>;

/// Name -> [`AggFactory`] table. Plain names, no folder prefix.
#[derive(Default)]
pub struct AggRegistry {
    fns: TFxMap<String, AggFactory>,
}

impl AggRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            fns: TFxMap::default(),
        }
    }

    /// Register a factory under `name` (last-wins on collision).
    pub fn register(&mut self, name: impl Into<String>, factory: AggFactory) {
        self.fns.insert(name.into(), factory);
    }

    /// Look up a factory by name.
    pub fn get(&self, name: &str) -> Option<&AggFactory> {
        self.fns.get(name)
    }

    /// Create a fresh aggregator instance by name.
    pub fn make(&self, name: &str) -> Option<Box<dyn Aggregator>> {
        self.fns.get(name).map(|f| f())
    }

    /// All registered aggregate names (unordered).
    pub fn names(&self) -> Vec<&str> {
        self.fns.keys().map(String::as_str).collect()
    }
}

// ---------------------------------------------------------------------------
// Registration of built-in aggregators
// ---------------------------------------------------------------------------

/// Register all built-in aggregators into `reg`.
pub fn register(reg: &mut AggRegistry) {
    reg.register("count", Arc::new(|| Box::new(CountAgg::new())));
    reg.register(
        "count_distinct",
        Arc::new(|| Box::new(CountDistinctAgg::new())),
    );
    reg.register("sum", Arc::new(|| Box::new(SumAgg::new())));
    reg.register("avg", Arc::new(|| Box::new(AvgAgg::new())));
    reg.register("min", Arc::new(|| Box::new(MinAgg::new())));
    reg.register("max", Arc::new(|| Box::new(MaxAgg::new())));
    reg.register("median", Arc::new(|| Box::new(MedianAgg::new())));
    reg.register("stddev", Arc::new(|| Box::new(StddevAgg::new())));
    reg.register("variance", Arc::new(|| Box::new(VarianceAgg::new())));
    // percentile default: p = 0.5 (equivalent to median)
    reg.register("percentile", percentile(0.5));
    reg.register("first", Arc::new(|| Box::new(FirstAgg::new())));
    reg.register("last", Arc::new(|| Box::new(LastAgg::new())));
    // string_agg default: sep = ","
    reg.register("string_agg", string_agg(",".to_owned()));
    reg.register("array_agg", Arc::new(|| Box::new(ArrayAggAgg::new())));
    reg.register("bool_and", Arc::new(|| Box::new(BoolAndAgg::new())));
    reg.register("bool_or", Arc::new(|| Box::new(BoolOrAgg::new())));
    reg.register("mode", Arc::new(|| Box::new(ModeAgg::new())));
    reg.register("range", Arc::new(|| Box::new(RangeAgg::new())));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Coerce a value to [`Decimal`] (same rules as `arg_dec` but takes a
/// ref instead of args+index).
fn to_dec(v: &QueryValue) -> Result<Decimal, ScalarError> {
    match v {
        QueryValue::Dec(d) => Ok(*d),
        QueryValue::Int(n) => Ok(Decimal::from(*n)),
        QueryValue::Bool(b) => Ok(Decimal::from(*b as i64)),
        QueryValue::F64(f) => rust_decimal::Decimal::from_f64_retain(*f)
            .ok_or_else(|| ScalarError::new("out_of_range")),
        QueryValue::Big(b) => {
            // Try i64 path first, then f64 fallback.
            if let Some(n) = b.to_i64() {
                Ok(Decimal::from(n))
            } else if let Some(f) = b.to_f64() {
                rust_decimal::Decimal::from_f64_retain(f)
                    .ok_or_else(|| ScalarError::new("out_of_range"))
            } else {
                Err(ScalarError::new("out_of_range"))
            }
        }
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

fn is_null(v: &QueryValue) -> bool {
    matches!(v, QueryValue::Null)
}

// ---------------------------------------------------------------------------
// count
// ---------------------------------------------------------------------------

struct CountAgg {
    n: i64,
}

impl CountAgg {
    fn new() -> Self {
        Self { n: 0 }
    }
}

impl Aggregator for CountAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if !is_null(v) {
            self.n += 1;
        }
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        Ok(QueryValue::Int(self.n))
    }
}

// ---------------------------------------------------------------------------
// count_distinct — uses compare for equality
// ---------------------------------------------------------------------------

struct CountDistinctAgg {
    seen: Vec<QueryValue>,
}

impl CountDistinctAgg {
    fn new() -> Self {
        Self { seen: Vec::new() }
    }
}

impl Aggregator for CountDistinctAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        let already = self
            .seen
            .iter()
            .any(|s| compare::compare(s, v) == std::cmp::Ordering::Equal);
        if !already {
            self.seen.push(v.clone());
        }
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        Ok(QueryValue::Int(self.seen.len() as i64))
    }
}

// ---------------------------------------------------------------------------
// sum
// ---------------------------------------------------------------------------

struct SumAgg {
    acc: Decimal,
    any: bool,
}

impl SumAgg {
    fn new() -> Self {
        Self {
            acc: Decimal::ZERO,
            any: false,
        }
    }
}

impl Aggregator for SumAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        self.acc += to_dec(v)?;
        self.any = true;
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        if !self.any {
            return Ok(QueryValue::Int(0));
        }
        Ok(QueryValue::Dec(self.acc))
    }
}

// ---------------------------------------------------------------------------
// avg
// ---------------------------------------------------------------------------

struct AvgAgg {
    sum: Decimal,
    count: i64,
}

impl AvgAgg {
    fn new() -> Self {
        Self {
            sum: Decimal::ZERO,
            count: 0,
        }
    }
}

impl Aggregator for AvgAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        self.sum += to_dec(v)?;
        self.count += 1;
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        if self.count == 0 {
            return Err(ScalarError::new("empty"));
        }
        Ok(QueryValue::Dec(self.sum / Decimal::from(self.count)))
    }
}

// ---------------------------------------------------------------------------
// min (cross-type via compare)
// ---------------------------------------------------------------------------

struct MinAgg {
    val: Option<QueryValue>,
}

impl MinAgg {
    fn new() -> Self {
        Self { val: None }
    }
}

impl Aggregator for MinAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        self.val = Some(match self.val.take() {
            None => v.clone(),
            Some(cur) => {
                if compare::compare(v, &cur) == std::cmp::Ordering::Less {
                    v.clone()
                } else {
                    cur
                }
            }
        });
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        self.val.ok_or_else(|| ScalarError::new("empty"))
    }
}

// ---------------------------------------------------------------------------
// max (cross-type via compare)
// ---------------------------------------------------------------------------

struct MaxAgg {
    val: Option<QueryValue>,
}

impl MaxAgg {
    fn new() -> Self {
        Self { val: None }
    }
}

impl Aggregator for MaxAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        self.val = Some(match self.val.take() {
            None => v.clone(),
            Some(cur) => {
                if compare::compare(v, &cur) == std::cmp::Ordering::Greater {
                    v.clone()
                } else {
                    cur
                }
            }
        });
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        self.val.ok_or_else(|| ScalarError::new("empty"))
    }
}

// ---------------------------------------------------------------------------
// median (collect + sort via compare)
// ---------------------------------------------------------------------------

struct MedianAgg {
    vals: Vec<QueryValue>,
}

impl MedianAgg {
    fn new() -> Self {
        Self { vals: Vec::new() }
    }
}

impl Aggregator for MedianAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if !is_null(v) {
            self.vals.push(v.clone());
        }
        Ok(())
    }

    fn finalize(mut self: Box<Self>) -> Result<QueryValue, ScalarError> {
        if self.vals.is_empty() {
            return Err(ScalarError::new("empty"));
        }
        self.vals.sort_by(compare::compare);
        let n = self.vals.len();
        // For odd n: middle element. For even n: lower-median (index n/2-1)
        // to avoid requiring numeric averaging of potentially non-numeric values.
        let mid = if n % 2 == 1 { n / 2 } else { n / 2 - 1 };
        Ok(self.vals.swap_remove(mid))
    }
}

// ---------------------------------------------------------------------------
// stddev (population)
// ---------------------------------------------------------------------------

struct StddevAgg {
    vals: Vec<Decimal>,
}

impl StddevAgg {
    fn new() -> Self {
        Self { vals: Vec::new() }
    }
}

impl Aggregator for StddevAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        self.vals.push(to_dec(v)?);
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        if self.vals.is_empty() {
            return Err(ScalarError::new("empty"));
        }
        let variance = compute_variance(&self.vals)?;
        let f = variance.to_f64().unwrap_or(f64::NAN);
        let sd = f.sqrt();
        Decimal::from_f64_retain(sd)
            .map(QueryValue::Dec)
            .ok_or_else(|| ScalarError::new("out_of_range"))
    }
}

// ---------------------------------------------------------------------------
// variance (population)
// ---------------------------------------------------------------------------

struct VarianceAgg {
    vals: Vec<Decimal>,
}

impl VarianceAgg {
    fn new() -> Self {
        Self { vals: Vec::new() }
    }
}

impl Aggregator for VarianceAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        self.vals.push(to_dec(v)?);
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        if self.vals.is_empty() {
            return Err(ScalarError::new("empty"));
        }
        let v = compute_variance(&self.vals)?;
        Ok(QueryValue::Dec(v))
    }
}

fn compute_variance(vals: &[Decimal]) -> Result<Decimal, ScalarError> {
    let n = Decimal::from(vals.len() as i64);
    let mean = vals.iter().copied().sum::<Decimal>() / n;
    let sum_sq: Decimal = vals.iter().map(|x| (*x - mean) * (*x - mean)).sum();
    Ok(sum_sq / n)
}

// ---------------------------------------------------------------------------
// percentile (parameterised p in [0,1])
// ---------------------------------------------------------------------------

/// Create a factory for the percentile aggregator with the given `p`.
pub fn percentile(p: f64) -> AggFactory {
    Arc::new(move || Box::new(PercentileAgg::new(p)))
}

struct PercentileAgg {
    p: f64,
    vals: Vec<QueryValue>,
}

impl PercentileAgg {
    fn new(p: f64) -> Self {
        Self {
            p,
            vals: Vec::new(),
        }
    }
}

impl Aggregator for PercentileAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if !is_null(v) {
            self.vals.push(v.clone());
        }
        Ok(())
    }

    fn finalize(mut self: Box<Self>) -> Result<QueryValue, ScalarError> {
        if self.vals.is_empty() {
            return Err(ScalarError::new("empty"));
        }
        if !(0.0..=1.0).contains(&self.p) {
            return Err(ScalarError::new("out_of_range"));
        }
        self.vals.sort_by(compare::compare);
        let n = self.vals.len();
        // Nearest-rank method.
        let idx = ((self.p * n as f64).ceil() as usize)
            .saturating_sub(1)
            .min(n - 1);
        Ok(self.vals.swap_remove(idx))
    }
}

// ---------------------------------------------------------------------------
// first / last
// ---------------------------------------------------------------------------

struct FirstAgg {
    val: Option<QueryValue>,
}

impl FirstAgg {
    fn new() -> Self {
        Self { val: None }
    }
}

impl Aggregator for FirstAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        if self.val.is_none() {
            self.val = Some(v.clone());
        }
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        self.val.ok_or_else(|| ScalarError::new("empty"))
    }
}

struct LastAgg {
    val: Option<QueryValue>,
}

impl LastAgg {
    fn new() -> Self {
        Self { val: None }
    }
}

impl Aggregator for LastAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        self.val = Some(v.clone());
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        self.val.ok_or_else(|| ScalarError::new("empty"))
    }
}

// ---------------------------------------------------------------------------
// string_agg (parameterised separator)
// ---------------------------------------------------------------------------

/// Create a factory for string_agg with the given separator.
pub fn string_agg(sep: String) -> AggFactory {
    Arc::new(move || Box::new(StringAggAgg::new(sep.clone())))
}

struct StringAggAgg {
    sep: String,
    parts: Vec<String>,
}

impl StringAggAgg {
    fn new(sep: String) -> Self {
        Self {
            sep,
            parts: Vec::new(),
        }
    }
}

impl Aggregator for StringAggAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        match v {
            QueryValue::Str(s) => {
                self.parts.push(s.clone());
                Ok(())
            }
            _ => Err(ScalarError::new("type_mismatch")),
        }
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        Ok(QueryValue::Str(self.parts.join(&self.sep)))
    }
}

// ---------------------------------------------------------------------------
// array_agg
// ---------------------------------------------------------------------------

struct ArrayAggAgg {
    items: Vec<QueryValue>,
}

impl ArrayAggAgg {
    fn new() -> Self {
        Self { items: Vec::new() }
    }
}

impl Aggregator for ArrayAggAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        // array_agg includes Nulls (collects everything).
        self.items.push(v.clone());
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        Ok(QueryValue::List(self.items))
    }
}

// ---------------------------------------------------------------------------
// bool_and / bool_or
// ---------------------------------------------------------------------------

struct BoolAndAgg {
    result: bool,
    any: bool,
}

impl BoolAndAgg {
    fn new() -> Self {
        Self {
            result: true,
            any: false,
        }
    }
}

impl Aggregator for BoolAndAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        match v {
            QueryValue::Bool(b) => {
                self.result = self.result && *b;
                self.any = true;
                Ok(())
            }
            _ => Err(ScalarError::new("type_mismatch")),
        }
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        // Identity for AND is true; return true even on empty input.
        let _ = self.any;
        Ok(QueryValue::Bool(self.result))
    }
}

struct BoolOrAgg {
    result: bool,
    any: bool,
}

impl BoolOrAgg {
    fn new() -> Self {
        Self {
            result: false,
            any: false,
        }
    }
}

impl Aggregator for BoolOrAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        match v {
            QueryValue::Bool(b) => {
                self.result = self.result || *b;
                self.any = true;
                Ok(())
            }
            _ => Err(ScalarError::new("type_mismatch")),
        }
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        // Identity for OR is false; return false even on empty input.
        let _ = self.any;
        Ok(QueryValue::Bool(self.result))
    }
}

// ---------------------------------------------------------------------------
// mode (most frequent via compare)
// ---------------------------------------------------------------------------

struct ModeAgg {
    vals: Vec<QueryValue>,
}

impl ModeAgg {
    fn new() -> Self {
        Self { vals: Vec::new() }
    }
}

impl Aggregator for ModeAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if !is_null(v) {
            self.vals.push(v.clone());
        }
        Ok(())
    }

    fn finalize(mut self: Box<Self>) -> Result<QueryValue, ScalarError> {
        if self.vals.is_empty() {
            return Err(ScalarError::new("empty"));
        }
        // Sort so equal values are adjacent, then run-length count.
        self.vals.sort_by(compare::compare);
        let mut best = self.vals[0].clone();
        let mut best_count: usize = 0;
        let mut cur_count: usize = 1;
        for i in 1..self.vals.len() {
            if compare::compare(&self.vals[i], &self.vals[i - 1]) == std::cmp::Ordering::Equal {
                cur_count += 1;
            } else {
                if cur_count > best_count {
                    best_count = cur_count;
                    best = self.vals[i - 1].clone();
                }
                cur_count = 1;
            }
        }
        // Final run.
        if cur_count > best_count {
            best = self.vals[self.vals.len() - 1].clone();
        }
        Ok(best)
    }
}

// ---------------------------------------------------------------------------
// range (max - min, numeric)
// ---------------------------------------------------------------------------

struct RangeAgg {
    min: Option<Decimal>,
    max: Option<Decimal>,
}

impl RangeAgg {
    fn new() -> Self {
        Self {
            min: None,
            max: None,
        }
    }
}

impl Aggregator for RangeAgg {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if is_null(v) {
            return Ok(());
        }
        let d = to_dec(v)?;
        self.min = Some(match self.min {
            Some(cur) => cur.min(d),
            None => d,
        });
        self.max = Some(match self.max {
            Some(cur) => cur.max(d),
            None => d,
        });
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        match (self.min, self.max) {
            (Some(lo), Some(hi)) => Ok(QueryValue::Dec(hi - lo)),
            _ => Ok(QueryValue::Int(0)),
        }
    }
}

#[cfg(test)]
mod tests;
