//! [`BatchLimits`] — security / resource limits for a single batch execution.

use serde::{Deserialize, Serialize};

/// Execution limits for security.
///
/// Prevents DoS attacks and resource exhaustion.
///
/// # Default Values
///
/// | Limit | Default | Description |
/// |-------|---------|-------------|
/// | `max_queries` | 50 | Maximum queries per batch |
/// | `max_dependency_depth` | 10 | Maximum dependency chain length |
/// | `max_execution_time_secs` | 30 | Maximum total execution time |
/// | `max_result_size` | 10MB | Maximum total result size |
/// | `max_iterations` | 1000 | Maximum `for_each` loop iterations |
///
/// # Example
///
/// ```text
/// {
///   "limits": {
///     "max_queries": 20,
///     "max_dependency_depth": 5,
///     "max_execution_time_secs": 10,
///     "max_result_size": 1000000
///   }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchLimits {
    /// Maximum number of queries in batch.
    pub max_queries: usize,

    /// Maximum dependency depth.
    ///
    /// A chain like a -> b -> c has depth 2.
    pub max_dependency_depth: usize,

    /// Maximum total execution time (seconds).
    pub max_execution_time_secs: u64,

    /// Maximum result size (bytes).
    pub max_result_size: usize,

    /// Maximum sub-batch nesting depth. 0 = no nesting allowed.
    pub max_nesting_depth: usize,

    /// Maximum `for_each` loop iterations (Epic04, #653).
    ///
    /// Bounds the repetition count of a `BatchOp::ForEach` body — the
    /// actual DoS backstop is the *product* `iterations × body.len()`, not
    /// this limit alone (see `BatchPlanner::plan`'s static gate, which folds
    /// that product into the existing `max_queries` budget when `over` is a
    /// literal array). This ceiling exists so a *dynamic* `over` (a
    /// `$query`-column-ref whose length is only known at runtime) still has
    /// a hard, finite bound, checked immediately before iteration 0 — see
    /// `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md` Decision 3.
    ///
    /// `#[serde(default = ...)]` (#662): this field was added after clients
    /// already shipped `limits` maps with only the original 5 fields. Without
    /// a default, serde makes it mandatory and rejects every such payload
    /// with `"missing field \`max_iterations\`"`. Defaulting to the same
    /// `1000` as [`BatchLimits::default`] keeps older/partial `limits`
    /// payloads (e.g. the TS client before it learns this field) wire-compatible.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

fn default_max_iterations() -> usize {
    1000
}

impl Default for BatchLimits {
    fn default() -> Self {
        BatchLimits {
            max_queries: 50,
            max_dependency_depth: 10,
            max_execution_time_secs: 30,
            max_result_size: 10 * 1024 * 1024, // 10MB
            max_nesting_depth: 4,
            max_iterations: 1000,
        }
    }
}
