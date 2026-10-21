use std::cell::RefCell;
use std::future::Future;

use crate::query::batch::{BatchError, BatchOp, QueryEntry};
use crate::query::read::QueryResult;
use shamir_query_types::batch::{TransactionInfo, VisibilityState};
use shamir_query_types::read::{VisibilityDependencyCode, VisibilityDependencyPending};
use shamir_types::types::value::QueryValue;

tokio::task_local! {
    static PENDING: RefCell<Vec<(String, u64, u64)>>;
}

/// Request-owned, lazily allocated only on Pending. Shared by nested bodies
/// and loop iterations; no RefCell borrow survives a suspension point.
pub(in crate::query::batch) async fn scope<T>(future: impl Future<Output = T>) -> T {
    if PENDING.try_with(|_| ()).is_ok() {
        future.await
    } else {
        PENDING.scope(RefCell::new(Vec::new()), future).await
    }
}

pub(in crate::query::batch) fn record(repo: &str, receipt: &TransactionInfo) {
    if let Some((version, floor)) = position(receipt) {
        let _ = PENDING.try_with(|pending| {
            let mut pending = pending.borrow_mut();
            if let Some(saved) = pending.iter_mut().find(|saved| saved.0 == repo) {
                saved.1 = saved.1.max(version);
                saved.2 = saved.2.min(floor);
            } else {
                pending.push((repo.to_owned(), version, floor));
            }
        });
    }
}

pub(in crate::query::batch) fn required(entry: &QueryEntry) -> Option<(u64, u64)> {
    if !matches!(&entry.op, BatchOp::Read(_)) {
        return None;
    }
    let repo = &entry.op.table_ref()?.repo;
    PENDING
        .try_with(|pending| {
            pending
                .borrow()
                .iter()
                .find(|saved| &saved.0 == repo)
                .map(|saved| (saved.1, saved.2))
        })
        .ok()
        .flatten()
}

pub(in crate::query::batch) fn failure(result: &QueryResult) -> Option<(u64, u64)> {
    failure_value(result.value.as_ref()?)
}

fn failure_value(value: &QueryValue) -> Option<(u64, u64)> {
    match value {
        QueryValue::List(values) => values.iter().find_map(failure_value),
        QueryValue::Map(fields) => {
            if matches!(fields.get("code"), Some(QueryValue::Str(code))
                if code == "visibility_dependency_pending")
            {
                return Some((
                    unsigned(fields.get("commit_version")?)?,
                    unsigned(fields.get("visible_through")?)?,
                ));
            }
            fields.values().find_map(|envelope| {
                let QueryValue::Map(envelope) = envelope else {
                    return None;
                };
                envelope.get("value").and_then(failure_value)
            })
        }
        _ => None,
    }
}

pub(in crate::query::batch) fn failure_result(
    position: (u64, u64),
) -> Result<QueryResult, BatchError> {
    let failure = VisibilityDependencyPending {
        code: VisibilityDependencyCode::VisibilityDependencyPending,
        commit_version: position.0,
        visible_through: position.1,
    };
    let value = crate::query::batch::query_value_serializer::to_query_value(&failure)
        .map_err(|e| BatchError::query_coded("", "codec", e.to_string()))?;
    Ok(QueryResult {
        skipped: true,
        ..QueryResult::with_value(value)
    })
}

pub(in crate::query::batch) fn has_receipt(result: &QueryResult) -> bool {
    result.receipt.is_some()
        || failure(result).is_some()
        || result.value.as_ref().is_some_and(has_nested_receipt)
}

fn has_nested_receipt(value: &QueryValue) -> bool {
    match value {
        QueryValue::List(values) => values.iter().any(has_nested_receipt),
        QueryValue::Map(aliases) => aliases.values().any(|value| {
            let QueryValue::Map(fields) = value else {
                return false;
            };
            fields.contains_key("receipt") || fields.get("value").is_some_and(has_nested_receipt)
        }),
        _ => false,
    }
}

/// Nested receipts live in the existing container value, not a synthetic
/// top-level transaction. Inspect only result envelopes, never arbitrary rows.
pub(in crate::query::batch) fn pending(result: &QueryResult) -> Option<(u64, u64)> {
    result
        .receipt
        .as_ref()
        .and_then(position)
        .or_else(|| result.value.as_ref().and_then(nested))
}

fn position(receipt: &TransactionInfo) -> Option<(u64, u64)> {
    match receipt.visibility? {
        VisibilityState::Pending {
            visible_through, ..
        } => Some((receipt.commit_version?, visible_through)),
        VisibilityState::Visible => None,
    }
}

fn nested(value: &QueryValue) -> Option<(u64, u64)> {
    match value {
        QueryValue::List(iterations) => iterations.iter().find_map(nested),
        QueryValue::Map(aliases) => aliases.values().find_map(|envelope| {
            let QueryValue::Map(fields) = envelope else {
                return None;
            };
            if let Some(QueryValue::Map(receipt)) = fields.get("receipt") {
                if let Some(QueryValue::Map(visibility)) = receipt.get("visibility") {
                    if matches!(visibility.get("state"), Some(QueryValue::Str(state))
                        if state == "pending")
                    {
                        return Some((
                            unsigned(receipt.get("commit_version")?)?,
                            unsigned(visibility.get("visible_through")?)?,
                        ));
                    }
                }
            }
            fields.get("value").and_then(nested)
        }),
        _ => None,
    }
}

fn unsigned(value: &QueryValue) -> Option<u64> {
    match value {
        QueryValue::Int(n) => u64::try_from(*n).ok(),
        QueryValue::Big(n) => n.to_string().parse().ok(),
        _ => None,
    }
}
