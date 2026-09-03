use crate::query::batch::executor_traits::TableResolver;
use crate::query::batch::{BatchError, BatchOp, QueryEntry};
use shamir_collections::TFxSet;
use shamir_types::types::common::TMap;

/// Validate that all referenced tables exist before execution.
///
/// Fails fast with a clear error if any table is not found, rather than
/// discovering it mid-execution after some operations have already run.
///
/// Tables/repos that are **created** by this same batch are exempted:
/// the DDL will materialise them before the DML runs (enforced by
/// `after` ordering edges).
pub(super) async fn validate_tables(
    queries: &TMap<String, QueryEntry>,
    resolver: &dyn TableResolver,
) -> Result<(), BatchError> {
    // Phase 1: collect tables/repos being created in this batch so we
    // can skip the existence check for them.
    let (created_tables, created_repos) = tables_created_in_batch(queries);

    // Phase 2: validate remaining table refs.
    let mut seen = shamir_types::types::common::new_set::<String>();
    for (alias, entry) in queries {
        if let Some(table_ref) = entry.op.table_ref() {
            let key = format!("{}/{}", table_ref.repo, table_ref.table);

            // Skip if this table is created by the batch itself.
            if created_tables.contains(&key) || created_repos.contains(&table_ref.repo) {
                continue;
            }

            if seen.insert(key) {
                resolver
                    .resolve(table_ref)
                    .await
                    .map_err(|e| BatchError::QueryError {
                        alias: alias.clone(),
                        message: format!(
                            "Table '{}' in repo '{}' not found: {}",
                            table_ref.table, table_ref.repo, e
                        ),
                        code: None,
                    })?;
            }
        }
    }
    Ok(())
}

/// Scan batch entries and return:
///  - `created_tables`: set of `"repo/table"` keys for `CreateTable` ops.
///  - `created_repos`: set of repo names for `CreateRepo` ops (any table
///    inside that repo is implicitly "being created").
pub(super) fn tables_created_in_batch(
    queries: &TMap<String, QueryEntry>,
) -> (TFxSet<String>, TFxSet<String>) {
    let mut created_tables = TFxSet::default();
    let mut created_repos = TFxSet::default();

    for entry in queries.values() {
        match &entry.op {
            BatchOp::CreateTable(ct) => {
                let key = format!("{}/{}", ct.repo, ct.create_table);
                created_tables.insert(key);
            }
            BatchOp::CreateRepo(cr) => {
                created_repos.insert(cr.create_repo.clone());
            }
            _ => {}
        }
    }

    (created_tables, created_repos)
}

/// Collect every top-level `Filter` tree attached to `entry` that the
/// engine will compile: the op's own WHERE/HAVING clause(s) plus the
/// entry's `when` guard (compiled unchecked by `query_runner::resolve_skip`
/// otherwise). Shared by [`validate_filter_depth`] and
/// [`validate_filter_patterns`] so the two validation passes can never
/// drift on which filters are covered — before this helper existed,
/// `Read.group_by.having` and `entry.when` were compiled with NO depth or
/// pattern check at all.
fn collect_entry_filters(entry: &QueryEntry) -> Vec<&shamir_query_types::filter::Filter> {
    let mut filters: Vec<&shamir_query_types::filter::Filter> = match &entry.op {
        BatchOp::Read(q) => {
            let mut fs: Vec<&shamir_query_types::filter::Filter> = q.r#where.iter().collect();
            if let Some(group_by) = &q.group_by {
                fs.extend(group_by.having.iter());
            }
            fs
        }
        BatchOp::Delete(d) => vec![&d.where_clause],
        BatchOp::Update(u) => u.where_clause.iter().collect(),
        _ => vec![],
    };
    filters.extend(entry.when.iter());
    filters
}

/// Validate that no filter in the batch exceeds the nesting depth cap.
///
/// Covers the op's own WHERE/HAVING clause AND `entry.when` (a `Filter`
/// that `query_runner::resolve_skip` compiles unconditionally — before this
/// fix, an over-deep `when`/`having` bypassed this guard entirely and could
/// blow the stack inside `compile_filter`/`resolve_filter_query` at
/// execution time; see `check_filter_depth`'s doc for the full mechanism,
/// including the "shallow Filter, deep FilterValue" variant).
pub(super) fn validate_filter_depth(queries: &TMap<String, QueryEntry>) -> Result<(), BatchError> {
    for (alias, entry) in queries {
        for f in collect_entry_filters(entry) {
            if let Err(e) = shamir_query_types::filter::check_filter_depth(f) {
                return Err(BatchError::QueryError {
                    alias: alias.clone(),
                    message: e,
                    code: None,
                });
            }
        }
    }
    Ok(())
}

/// Validate that every `Regex`/`Like`/`ILike` pattern in the batch (WHERE,
/// HAVING, `when`) is within the length cap and compiles successfully.
///
/// Without this, `compile_filter` silently folds an invalid pattern to
/// `FilterNode::False` — which, under a `NOT`, becomes effectively `True`
/// (e.g. `DELETE ... WHERE NOT (regex-with-a-typo)` matches every row) with
/// no error surfaced to the caller. This runs in the same validation pass
/// as [`validate_filter_depth`], over the same filter set.
pub(super) fn validate_filter_patterns(
    queries: &TMap<String, QueryEntry>,
) -> Result<(), BatchError> {
    for (alias, entry) in queries {
        for f in collect_entry_filters(entry) {
            if let Err(message) = crate::query::filter::check_filter_patterns(f) {
                return Err(BatchError::QueryError {
                    alias: alias.clone(),
                    message,
                    code: None,
                });
            }
        }
    }
    Ok(())
}
