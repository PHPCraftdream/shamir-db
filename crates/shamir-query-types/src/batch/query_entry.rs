//! [`QueryEntry`] — a single operation slot inside a [`BatchRequest`], plus
//! the [`distinct_repos`] / [`collect_required_access`] helpers that inspect
//! a map of entries.

use serde::{Deserialize, Serialize};
use shamir_collections::{TFxSet, TMap};
use shamir_types::access::{Action, ResourcePath};

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

/// Recursively collect every `(Action, ResourcePath)` authorization
/// requirement across the WHOLE query tree — including inside nested
/// `Batch`/`ForEach` bodies, at any depth. This is the single source of
/// truth the per-op authorization pre-check loops (`ShamirDb::execute_as` /
/// `tx_execute_as`) must use instead of a flat, one-level walk — a flat
/// walk sees `None` for `Batch`/`ForEach` (they have no `table_ref()`) and
/// silently skips whatever tables their nested body actually touches (the
/// #660-class bug, but for authorization instead of repo detection: an
/// actor with permission on SOME tables but not a forbidden one could
/// read/write the forbidden table by simply wrapping the op in a
/// top-level `Batch`/`ForEach`, since the inner op's own
/// `required_access()` was never consulted).
pub fn collect_required_access(
    queries: &TMap<String, QueryEntry>,
    db: &str,
) -> Vec<(Action, ResourcePath)> {
    let mut out = Vec::new();
    collect_required_access_into(queries, db, &mut out);
    out
}

/// Recursive collector behind [`collect_required_access`]: adds each
/// entry's `required_access()` requirement (when present) and descends
/// into `Batch`/`ForEach` bodies' `queries` maps so nested levels
/// contribute too — mirrors [`collect_repos`]'s recursion shape exactly.
fn collect_required_access_into(
    queries: &TMap<String, QueryEntry>,
    db: &str,
    out: &mut Vec<(Action, ResourcePath)>,
) {
    for qe in queries.values() {
        if let Some(req) = qe.op.required_access(db) {
            out.push(req);
        }
        match &qe.op {
            BatchOp::Batch(sub) => collect_required_access_into(&sub.batch.queries, db, out),
            BatchOp::ForEach(fe) => collect_required_access_into(&fe.batch.queries, db, out),
            _ => {}
        }
    }
}
