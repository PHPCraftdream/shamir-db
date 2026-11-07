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
/// | `max_result_size` | 10 MiB | Maximum total result size |
/// | `max_nesting_depth` | 4 | Maximum sub-batch nesting depth |
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
#[serde(deny_unknown_fields)]
pub struct BatchLimits {
    /// Maximum number of queries in batch.
    #[serde(default = "default_max_queries")]
    pub max_queries: usize,

    /// Maximum dependency depth.
    ///
    /// A chain like a -> b -> c has depth 2.
    #[serde(default = "default_max_dependency_depth")]
    pub max_dependency_depth: usize,

    /// Maximum total execution time (seconds).
    #[serde(default = "default_max_execution_time_secs")]
    pub max_execution_time_secs: u64,

    /// Maximum result size (bytes).
    #[serde(default = "default_max_result_size")]
    pub max_result_size: usize,

    /// Maximum sub-batch nesting depth. 0 = no nesting allowed.
    #[serde(default = "default_max_nesting_depth")]
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

    /// Maximum total queries across the batch tree.
    #[serde(default = "default_max_total_queries")]
    pub max_total_queries: usize,

    /// Maximum total work across repeated and nested operations.
    #[serde(default = "default_max_total_work")]
    pub max_total_work: u64,

    /// Maximum requested page size.
    #[serde(default = "default_max_page_size")]
    pub max_page_size: u64,

    /// Maximum requested pagination offset.
    #[serde(default = "default_max_offset")]
    pub max_offset: u64,
}

fn default_max_queries() -> usize {
    50
}
fn default_max_dependency_depth() -> usize {
    10
}
fn default_max_execution_time_secs() -> u64 {
    30
}
fn default_max_result_size() -> usize {
    10 * 1024 * 1024
}
fn default_max_nesting_depth() -> usize {
    4
}

fn default_max_iterations() -> usize {
    1000
}

fn default_max_total_queries() -> usize {
    10_000
}

fn default_max_total_work() -> u64 {
    1_000_000
}

fn default_max_page_size() -> u64 {
    100_000
}

fn default_max_offset() -> u64 {
    1_000_000
}

impl BatchLimits {
    /// Hard engine caps; max_queries = 1000 keeps the planner's recursive DFS shallow.
    pub const CEILING: BatchLimits = BatchLimits {
        max_queries: 1000,
        max_dependency_depth: 1000,
        max_execution_time_secs: 3600,
        max_result_size: 100 * 1024 * 1024,
        max_nesting_depth: 64,
        max_iterations: 10_000,
        max_total_queries: 10_000,
        max_total_work: 1_000_000,
        max_page_size: 100_000,
        max_offset: 1_000_000,
    };

    /// Lower every numeric request limit to the corresponding policy cap.
    pub fn clamped_to(&self, cap: &BatchLimits) -> BatchLimits {
        BatchLimits {
            max_queries: self.max_queries.min(cap.max_queries),
            max_dependency_depth: self.max_dependency_depth.min(cap.max_dependency_depth),
            max_execution_time_secs: self
                .max_execution_time_secs
                .min(cap.max_execution_time_secs),
            max_result_size: self.max_result_size.min(cap.max_result_size),
            max_nesting_depth: self.max_nesting_depth.min(cap.max_nesting_depth),
            max_iterations: self.max_iterations.min(cap.max_iterations),
            max_total_queries: self.max_total_queries.min(cap.max_total_queries),
            max_total_work: self.max_total_work.min(cap.max_total_work),
            max_page_size: self.max_page_size.min(cap.max_page_size),
            max_offset: self.max_offset.min(cap.max_offset),
        }
    }
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
            max_total_queries: default_max_total_queries(),
            max_total_work: default_max_total_work(),
            max_page_size: default_max_page_size(),
            max_offset: default_max_offset(),
        }
    }
}
