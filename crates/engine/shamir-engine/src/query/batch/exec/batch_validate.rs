use crate::query::batch::executor_traits::TableResolver;
use crate::query::batch::{BatchError, BatchOp, QueryEntry};
use crate::query::filter::FilterValue;
use shamir_collections::{TFxSet, TMap};
use shamir_query_types::batch::{BatchLimits, BatchTreeVisitor, LimitKind};
use shamir_query_types::filter::Filter;
use shamir_query_types::read::{ReadQuery, SelectExpr, SelectItem};

/// Validate that all referenced tables exist before execution.
///
/// Fails fast with a clear error if any table is not found, rather than
/// discovering it mid-execution after some operations have already run.
///
/// Tables/repos that are **created** by this same batch are exempted:
/// the DDL will materialise them before the DML runs (enforced by
/// `after` ordering edges).
pub(in crate::query::batch) async fn validate_tables(
    queries: &TMap<String, QueryEntry>,
    resolver: &dyn TableResolver,
    limits: &BatchLimits,
    root_limits: &BatchLimits,
) -> Result<(), BatchError> {
    if limits == root_limits {
        validate_pagination(queries, limits)?;
    } else {
        validate_pagination_with_root(queries, limits, root_limits)?;
    }
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

pub(in crate::query::batch) fn validate_pagination(
    queries: &TMap<String, QueryEntry>,
    limits: &BatchLimits,
) -> Result<(), BatchError> {
    validate_pagination_with_root(queries, limits, limits)
}

fn validate_pagination_with_root(
    queries: &TMap<String, QueryEntry>,
    limits: &BatchLimits,
    root_limits: &BatchLimits,
) -> Result<(), BatchError> {
    let mut stack = vec![(queries.values(), limits.clone())];
    while let Some((entries, limits)) = stack.last_mut() {
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        match &entry.op {
            BatchOp::Read(query) => {
                let (offset, limit) = query.pagination.try_resolve()?;
                if offset > limits.max_offset {
                    return Err(BatchError::LimitExceeded {
                        kind: LimitKind::Offset,
                        actual: offset,
                        max: limits.max_offset,
                    });
                }
                if let Some(limit) = limit {
                    if limit > limits.max_page_size {
                        return Err(BatchError::LimitExceeded {
                            kind: LimitKind::PageSize,
                            actual: limit,
                            max: limits.max_page_size,
                        });
                    }
                }
            }
            BatchOp::Batch(sub) => stack.push((
                sub.batch.queries.values(),
                sub.batch.limits.clamped_to(root_limits),
            )),
            BatchOp::ForEach(loop_op) => stack.push((
                loop_op.batch.queries.values(),
                loop_op.batch.limits.clamped_to(root_limits),
            )),
            _ => {}
        }
    }
    Ok(())
}

/// Scan batch entries and return:
///  - `created_tables`: set of `"repo/table"` keys for `CreateTable` ops.
///  - `created_repos`: set of repo names for `CreateRepo` ops (any table
///    inside that repo is implicitly "being created").
pub(in crate::query::batch) fn tables_created_in_batch(
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

/// Validate every executable filter/value/select-expression surface in all
/// nested batch and foreach bodies in one iterative batch-tree walk.
pub(in crate::query::batch) fn validate_executable_guards(
    queries: &TMap<String, QueryEntry>,
) -> Result<(), BatchError> {
    for visit in BatchTreeVisitor::new(queries) {
        let alias = visit.alias;
        validate_entry_surfaces(visit.entry, alias)?;
    }
    Ok(())
}

fn validate_entry_surfaces(entry: &QueryEntry, alias: &str) -> Result<(), BatchError> {
    let mut filters: Vec<&Filter> = entry.when.iter().collect();
    let mut values: Vec<&FilterValue> = Vec::new();
    let mut expressions: Vec<&SelectExpr> = Vec::new();
    match &entry.op {
        BatchOp::Read(read) => {
            collect_read_surfaces(read, &mut filters, &mut values, &mut expressions)
        }
        BatchOp::Delete(delete) => filters.push(&delete.where_clause),
        BatchOp::Update(update) => filters.extend(update.where_clause.iter()),
        BatchOp::Batch(sub) => values.extend(sub.bind.values()),
        BatchOp::ForEach(for_each) => values.push(&for_each.over),
        BatchOp::Call(call) => values.extend(&call.params),
        BatchOp::Subscribe(subscribe) => {
            filters.extend(
                subscribe
                    .subscribe
                    .iter()
                    .filter_map(|source| source.filter.as_ref()),
            );
        }
        BatchOp::SetTableSchema(op) => {
            values.extend(
                op.schema
                    .iter()
                    .filter_map(|rule| rule.constraints.default.as_ref()),
            );
        }
        BatchOp::AddSchemaRule(op) => {
            if let Some(default) = &op.rule.constraints.default {
                values.push(default);
            }
        }
        _ => {}
    }
    for filter in filters {
        validate_filter_surface(alias, filter)?;
    }
    for value in values {
        validate_value_surface(alias, value)?;
    }
    for expression in expressions {
        let depth = shamir_query_types::filter::check_select_expr_depth(expression);
        map_guard_result(alias, depth)?;
        let patterns = crate::query::filter::check_select_expr_patterns(expression);
        map_guard_result(alias, patterns)?;
    }
    Ok(())
}

fn collect_read_surfaces<'a>(
    read: &'a ReadQuery,
    filters: &mut Vec<&'a Filter>,
    values: &mut Vec<&'a FilterValue>,
    expressions: &mut Vec<&'a SelectExpr>,
) {
    filters.extend(read.r#where.iter());
    if let Some(group) = &read.group_by {
        filters.extend(group.having.iter());
    }
    for item in &read.select.items {
        match item {
            SelectItem::Aggregate { input, .. } => values.extend(input.iter()),
            SelectItem::AggregateFn { input, args, .. } => {
                values.extend(input.iter());
                values.extend(args);
            }
            SelectItem::Function { args, .. } => values.extend(args),
            SelectItem::Expression { expr, .. } => expressions.push(expr),
            SelectItem::All | SelectItem::Field { .. } | SelectItem::CountAll { .. } => {}
        }
    }
}

fn validate_filter_surface(alias: &str, filter: &Filter) -> Result<(), BatchError> {
    map_guard_result(
        alias,
        shamir_query_types::filter::check_filter_depth(filter),
    )?;
    map_guard_result(alias, crate::query::filter::check_filter_patterns(filter))
}

fn validate_value_surface(alias: &str, value: &FilterValue) -> Result<(), BatchError> {
    map_guard_result(
        alias,
        shamir_query_types::filter::check_filter_value_depth(value),
    )?;
    map_guard_result(
        alias,
        crate::query::filter::check_filter_value_patterns(value),
    )
}

fn map_guard_result(alias: &str, result: Result<(), String>) -> Result<(), BatchError> {
    result.map_err(|message| BatchError::QueryError {
        alias: alias.to_string(),
        message,
        code: None,
    })
}
