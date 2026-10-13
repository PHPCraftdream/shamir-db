//! Per-alias receipt extraction from an implicit (non-transactional)
//! batch response, including NESTED results of sub-batch / for-each
//! containers.

use shamir_query_types::batch::{BatchOp, BatchRequest, BatchResponse, QueryEntry};
use shamir_types::types::value::QueryValue;

use super::alias_receipt::AliasReceipt;
use super::nested_receipt::decode_nested_transaction;
use crate::visibility::CommitResult;
use crate::ClientError;

/// Extract every write alias's receipt from `response`, validating each
/// exactly like the transactional path (`TransactionInfo::validate`,
/// committed-with-visibility; malformed metadata → `ClientError::Protocol`,
/// aborted/indeterminate outcomes → `ClientError::TransactionOutcome`).
///
/// A suppressed write alias (`return_result: false`) is kept by the
/// server as a row-less shell that still carries its receipt — so this
/// reads `QueryResult.receipt` directly and never infers anything from
/// rows. Only aliases present in the REQUEST are considered at the top
/// level.
///
/// NESTED results are walked REQUEST-DRIVEN: descent happens ONLY under
/// a request entry whose op is a sub-batch (`BatchOp::Batch`) or a
/// for-each (`BatchOp::ForEach`), and the inner request entries decide
/// recursion and the inner alias names — the server stores sub-batch
/// results inside the container's `value` MAP (`alias -> QueryResult`)
/// and for-each results inside its `value` LIST (one iteration map per
/// iteration; see `query_runner.rs`). Nested receipts surface with a
/// path-style alias (`outer.inner`, `loop[3].w`) and `repo: None` — a
/// nested alias's repository cannot be derived unambiguously — so
/// reconciliation refuses them rather than guess. The `value` of ANY
/// other op (reads, calls, writes) is application data and is never
/// inspected; if a result does not match the request shape it is
/// skipped, never guessed.
pub fn extract_receipts(
    request: &BatchRequest,
    response: &BatchResponse,
) -> Result<Vec<AliasReceipt>, ClientError> {
    let mut receipts = Vec::new();
    for (alias, entry) in request.queries.iter() {
        let Some(result) = response.results.get(alias) else {
            continue;
        };
        let repo = entry.op.table_ref().map(|tr| tr.repo.clone());
        if let Some(transaction) = &result.receipt {
            let commit = CommitResult::from_transaction(transaction.clone())?;
            let crate::visibility::CommitResult::Committed {
                transaction,
                visibility,
            } = commit;
            receipts.push(AliasReceipt {
                alias: alias.to_string(),
                transaction,
                visibility,
                repo,
            });
        }
        walk_entry(alias, &entry.op, result.value.as_ref(), &mut receipts)?;
    }
    Ok(receipts)
}

/// Descend into `op`'s result only when the REQUEST op is a container;
/// recurse over the inner request entries. Anything else is application
/// data and is ignored.
fn walk_entry(
    prefix: &str,
    op: &BatchOp,
    value: Option<&QueryValue>,
    out: &mut Vec<AliasReceipt>,
) -> Result<(), ClientError> {
    match op {
        BatchOp::Batch(sub) => {
            let Some(QueryValue::Map(entries)) = value else {
                return Ok(()); // result does not match the request shape: skip
            };
            walk_result_map(prefix, &sub.batch.queries, entries, out)
        }
        BatchOp::ForEach(fe) => {
            let Some(QueryValue::List(iterations)) = value else {
                return Ok(()); // result does not match the request shape: skip
            };
            for (i, iteration) in iterations.iter().enumerate() {
                if let QueryValue::Map(entries) = iteration {
                    walk_result_map(&format!("{prefix}[{i}]"), &fe.batch.queries, entries, out)?;
                }
            }
            Ok(())
        }
        _ => Ok(()), // application value: never inspected
    }
}

/// Walk one nested results map against the inner REQUEST entries: the
/// request decides which aliases exist and which are containers. A nested
/// result that is not a map, or an alias the request does not declare, is
/// skipped.
fn walk_result_map(
    prefix: &str,
    queries: &shamir_collections::TMap<String, QueryEntry>,
    entries: &shamir_types::types::common::TMap<String, QueryValue>,
    out: &mut Vec<AliasReceipt>,
) -> Result<(), ClientError> {
    for (alias, entry) in queries.iter() {
        let Some(result) = entries.get(alias) else {
            continue;
        };
        let QueryValue::Map(result_map) = result else {
            continue;
        };
        let receipt = match result_map.get("receipt") {
            Some(r) if !matches!(r, QueryValue::Null) => Some(decode_nested_transaction(r)?),
            _ => None,
        };
        if let Some(transaction) = receipt {
            let commit = CommitResult::from_transaction(transaction)?;
            let crate::visibility::CommitResult::Committed {
                transaction,
                visibility,
            } = commit;
            out.push(AliasReceipt {
                alias: format!("{prefix}.{alias}"),
                transaction,
                visibility,
                repo: None,
            });
        }
        walk_entry(
            &format!("{prefix}.{alias}"),
            &entry.op,
            result_map.get("value"),
            out,
        )?;
    }
    Ok(())
}
