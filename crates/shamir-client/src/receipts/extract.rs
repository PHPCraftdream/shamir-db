//! Per-alias receipt extraction from an implicit (non-transactional)
//! batch response, including NESTED results.

use shamir_query_types::batch::{BatchRequest, BatchResponse};
use shamir_types::types::value::QueryValue;

use super::alias_receipt::AliasReceipt;
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
/// NESTED results: sub-batch containers keep their inner results inside
/// the container's `value` map (`alias -> QueryResult`,
/// `query_runner.rs`), and for-each containers inside its `value` LIST
/// (one iteration map of `alias -> QueryResult` per iteration). Nested
/// receipts are surfaced with a path-style alias (`outer.inner`,
/// `loop[3].w`) and `repo: None` — a nested alias's repository cannot be
/// derived unambiguously — so reconciliation refuses them rather than
/// guess. Nested receipts are decoded through the same strict
/// `TransactionInfo` validation; any result map carrying a `receipt` key
/// that decodes is treated as a receipt carrier (an application value
/// that happens to embed a well-formed committed TransactionInfo would
/// be surfaced too — the decode is exact, so this is not a guess).
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
        push_result(
            alias,
            &result.receipt,
            result.value.as_ref(),
            repo,
            &mut receipts,
        )?;
    }
    Ok(receipts)
}

/// Emit `alias`'s own receipt (if any) and descend into its `value`
/// container, if it is one.
fn push_result(
    alias: &str,
    receipt: &Option<shamir_query_types::batch::TransactionInfo>,
    value: Option<&QueryValue>,
    repo: Option<String>,
    out: &mut Vec<AliasReceipt>,
) -> Result<(), ClientError> {
    if let Some(transaction) = receipt {
        let commit = CommitResult::from_transaction(transaction.clone())?;
        let crate::visibility::CommitResult::Committed {
            transaction,
            visibility,
        } = commit;
        out.push(AliasReceipt {
            alias: alias.to_string(),
            transaction,
            visibility,
            repo,
        });
    }
    walk_container(alias, value, out)
}

/// Walk a container's `value`: a sub-batch stores a MAP of
/// `alias -> QueryResult`; a for-each stores a LIST of iteration MAPS of
/// `alias -> QueryResult`. Anything else is an application value and is
/// ignored.
fn walk_container(
    prefix: &str,
    value: Option<&QueryValue>,
    out: &mut Vec<AliasReceipt>,
) -> Result<(), ClientError> {
    let Some(value) = value else {
        return Ok(());
    };
    match value {
        QueryValue::Map(entries) => {
            for (alias, result) in entries {
                walk_nested(&format!("{prefix}.{alias}"), result, out)?;
            }
        }
        QueryValue::List(iterations) => {
            for (i, iteration) in iterations.iter().enumerate() {
                if let QueryValue::Map(entries) = iteration {
                    for (alias, result) in entries {
                        walk_nested(&format!("{prefix}[{i}].{alias}"), result, out)?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// A nested serialized `QueryResult`: decode its `receipt` (if any)
/// through the strict transaction validation and keep walking deeper
/// containers. Nested receipts always carry `repo: None`.
fn walk_nested(
    alias: &str,
    result: &QueryValue,
    out: &mut Vec<AliasReceipt>,
) -> Result<(), ClientError> {
    let QueryValue::Map(map) = result else {
        return Ok(());
    };
    let receipt = match map.get("receipt") {
        Some(r) if !matches!(r, QueryValue::Null) => Some(decode_nested_transaction(r)?),
        _ => None,
    };
    push_result(alias, &receipt, map.get("value"), None, out)
}

/// Reverse of the server's msgpack round-trip for one nested
/// `TransactionInfo`, with the same strict v3 validation.
fn decode_nested_transaction(
    receipt: &QueryValue,
) -> Result<shamir_query_types::batch::TransactionInfo, ClientError> {
    let bytes = rmp_serde::to_vec_named(receipt)
        .map_err(|e| ClientError::Protocol(format!("nested receipt encode: {e}")))?;
    rmp_serde::from_slice(&bytes).map_err(|e| ClientError::Protocol(format!("nested receipt: {e}")))
}
