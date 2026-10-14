use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::TransportKind;
use shamir_connect::server::session::Session;
use shamir_query_types::batch::{BatchOp, BatchRequest, BatchResponse};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use super::handler::ShamirDbHandler;

fn nested_result_map(
    op: &BatchOp,
    result: &shamir_query_types::read::QueryResult,
) -> Option<shamir_collections::TMap<String, shamir_query_types::read::QueryResult>> {
    let QueryValue::Map(fields) = result.value.as_ref()? else {
        return None;
    };
    let nested_value = match op {
        BatchOp::Batch(_) => fields.get("value")?,
        BatchOp::ForEach(_) => {
            let QueryValue::List(iterations) = fields.get("value")? else {
                return None;
            };
            if iterations.len() != 1 {
                return None;
            }
            &iterations[0]
        }
        _ => return None,
    };
    let bytes = rmp_serde::to_vec_named(nested_value).ok()?;
    rmp_serde::from_slice(&bytes).ok()
}

fn nested_queries(
    op: &BatchOp,
) -> Option<&shamir_collections::TMap<String, shamir_query_types::batch::QueryEntry>> {
    match op {
        BatchOp::Batch(sub) => Some(&sub.batch.queries),
        BatchOp::ForEach(loop_op) => Some(&loop_op.batch.queries),
        _ => None,
    }
}

mod result_check;
use result_check::is_successful_alter_result;

fn transport_name(kind: TransportKind) -> &'static str {
    match kind {
        TransportKind::Tcp => "tcp",
        TransportKind::WebSocket => "ws",
        TransportKind::Unix => "unix",
    }
}

impl ShamirDbHandler {
    pub(super) fn audit_successful_alter_repo(
        &self,
        session: &Session,
        db_name: &str,
        batch: &BatchRequest,
        response: &BatchResponse,
    ) {
        let Some(audit) = &self.audit else {
            return;
        };
        self.audit_query_tree(session, db_name, &batch.queries, &response.results, audit);
    }

    fn audit_query_tree(
        &self,
        session: &Session,
        db_name: &str,
        queries: &shamir_collections::TMap<String, shamir_query_types::batch::QueryEntry>,
        results: &shamir_collections::TMap<String, shamir_query_types::read::QueryResult>,
        audit: &shamir_connect::server::audit_chain::AuditChainWriter,
    ) {
        for (alias, entry) in queries {
            let BatchOp::AlterRepo(op) = &entry.op else {
                let Some(result) = results.get(alias) else {
                    continue;
                };
                if result.skipped {
                    continue;
                }
                let Some(child_results) = nested_result_map(&entry.op, result) else {
                    continue;
                };
                if let Some(child_queries) = nested_queries(&entry.op) {
                    self.audit_query_tree(session, db_name, child_queries, &child_results, audit);
                }
                continue;
            };
            let Some(result) = results.get(alias) else {
                continue;
            };
            if result.skipped || !is_successful_alter_result(result, &op.alter_repo) {
                continue;
            }
            let durability = match result.value.as_ref() {
                Some(shamir_types::types::value::QueryValue::Map(map)) => map
                    .get("durability")
                    .cloned()
                    .unwrap_or(shamir_types::types::value::QueryValue::Null),
                _ => shamir_types::types::value::QueryValue::Null,
            };
            let mut prefix = [0u8; 8];
            prefix.copy_from_slice(&session.session_id[..8]);
            let details = mpack!({
                "db": db_name,
                "repo": op.alter_repo,
                "alias": alias,
                "durability": @durability,
            });
            if let Ok(bytes) = rmp_serde::to_vec_named(&details) {
                audit.append(
                    "alter_repo",
                    transport_name(session.transport_kind),
                    session.username.clone(),
                    "",
                    prefix,
                    "ok",
                    bytes,
                    UnixNanos::now().as_u64(),
                );
            }
        }
    }
}
