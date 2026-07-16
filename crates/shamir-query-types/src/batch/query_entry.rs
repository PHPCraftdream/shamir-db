//! [`QueryEntry`] — a single operation slot inside a [`BatchRequest`], plus
//! the [`distinct_repos`] helper that inspects a map of entries.

use serde::{Deserialize, Serialize};
use shamir_collections::{TFxSet, TMap};

use crate::filter::Filter;
use crate::read::ReadQuery;

use super::batch_op::BatchOp;

/// Operation entry for batch requests.
///
/// Used as the value in the `queries` map where the key is the alias.
///
/// # Examples
///
/// ```text
/// // Query
/// { "from": "users", "where": { "op": "eq", "field": "status", "value": "active" } }
///
/// // Insert
/// { "insert_into": "users", "values": [{ "name": "Alice" }] }
///
/// // Update
/// { "update": "users", "where": { "op": "eq", "field": "id", "value": 1 }, "set": { "name": "Bob" } }
///
/// // Set (upsert)
/// { "set": "users", "key": { "id": 1 }, "value": { "name": "Charlie" } }
///
/// // Delete
/// { "delete_from": "users", "where": { "op": "eq", "field": "id", "value": 1 } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryEntry {
    /// The operation to execute (flattened for shorthand syntax).
    #[serde(flatten)]
    pub op: BatchOp,

    /// Whether to include this result in the response.
    ///
    /// - `true` (default): Include in `results`
    /// - `false`: Exclude (useful for intermediate queries)
    #[serde(default = "default_return")]
    pub return_result: bool,

    /// Explicit ordering dependencies: aliases (in this batch) that MUST
    /// execute before this entry. Complements the auto-extracted `$query`
    /// dependencies. Enables DDL→DML ordering (e.g. insert after create_table).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,

    /// Conditional-execution gate (Epic03/B, #645). When present, the op
    /// executes iff `filter` evaluates to `true` against an empty synthetic
    /// record (only `$query`/`$fn`/`$param`/literals are meaningful — see
    /// `docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`
    /// Decision 1). `None` (the default) is today's unconditional-execution
    /// behavior — omitted from the wire for full backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Filter>,
}

fn default_return() -> bool {
    true
}

impl From<ReadQuery> for QueryEntry {
    fn from(query: ReadQuery) -> Self {
        QueryEntry {
            op: BatchOp::Read(query),
            return_result: true,
            after: Vec::new(),
            when: None,
        }
    }
}

/// Returns the set of distinct repository names referenced by the
/// data queries in `queries`. Admin ops (which return `none` from
/// `BatchOp::table_ref`) do not contribute. `BatchOp::Batch`/`BatchOp::
/// ForEach` bodies ARE walked recursively (#660): a nested sub-batch/loop
/// body executes WITHIN the outer transaction (Epic04 ADR Decision 4 — an
/// iteration failure aborts the whole tx batch), so the repos its ops touch
/// genuinely participate in the OUTER batch's cross-repo scope and must be
/// visible to the cross-repo guard. Note `BatchOp::table_ref()` itself still
/// returns `None` for both variants — a single `Option<&TableRef>` cannot
/// express a nested body's multiple tables; the walk lives here.
///
/// Used by the executor to enforce the cross-repo guard for
/// transactional batches (Stage 4.C).
pub fn distinct_repos(queries: &TMap<String, QueryEntry>) -> TFxSet<String> {
    let mut repos = TFxSet::default();
    collect_repos(queries, &mut repos);
    repos
}

/// Recursive collector behind [`distinct_repos`]: adds each entry's
/// `table_ref()` repo (when present) and descends into `Batch`/`ForEach`
/// bodies' `queries` maps so nested levels contribute too.
fn collect_repos(queries: &TMap<String, QueryEntry>, repos: &mut TFxSet<String>) {
    for qe in queries.values() {
        if let Some(tr) = qe.op.table_ref() {
            repos.insert(tr.repo.clone());
        }
        match &qe.op {
            BatchOp::Batch(sub) => collect_repos(&sub.batch.queries, repos),
            BatchOp::ForEach(fe) => collect_repos(&fe.batch.queries, repos),
            _ => {}
        }
    }
}
