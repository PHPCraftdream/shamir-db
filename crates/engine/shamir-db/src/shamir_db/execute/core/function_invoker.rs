//! `FunctionInvoker` implementation for `ShamirDb`.

use crate::access::Actor;
use crate::query::batch::{BatchError, FunctionInvoker};
use crate::query::read::QueryResult;
use crate::shamir_db::shamir_db::GatewayCodeSink;
use crate::types::common::TMap;
use std::sync::Arc;

use super::super::shamir_db::ShamirDb;
use super::helpers::filter_value_to_query_value;

/// FunctionInvoker that invokes functions via `ShamirDb::invoke_function_in_db_as`.
pub(super) struct ShamirFunctionInvoker {
    pub(super) shamir: ShamirDb,
    pub(super) db_name: String,
}

#[async_trait::async_trait]
impl FunctionInvoker for ShamirFunctionInvoker {
    async fn invoke_call(
        &self,
        op: &crate::query::CallOp,
        actor: &Actor,
        resolved_refs: &TMap<String, QueryResult>,
    ) -> Result<QueryResult, BatchError> {
        // Convert positional Vec<FilterValue> params into Params, resolving
        // `$query` references against `resolved_refs` (Phase 2). Literals
        // pass through unchanged.
        //
        // Layout:
        //   - Each param at index i is stored under key "i" (positional access).
        //   - The full array is stored under key "args" as QueryValue::List.
        //
        // Guest SDK reads: `params.get("0")` for first arg, or
        // `params.get("args")` for the whole array.
        let mut params = crate::engine::function::Params::new();
        let mut args_list = Vec::with_capacity(op.params.len());
        for (i, fv) in op.params.iter().enumerate() {
            let qv = filter_value_to_query_value(fv, resolved_refs);
            let key: String = i.to_string();
            params.set(key, qv.clone());
            args_list.push(qv);
        }
        params.set("args", crate::types::value::QueryValue::List(args_list));

        // A fresh sink per call keeps an earlier invocation's code out of
        // this call's error (anti-stale).
        let code_sink = Arc::new(GatewayCodeSink::new());
        let qv = self
            .shamir
            .invoke_function_in_db_as_with_gateway_code(
                &self.db_name,
                &op.repo,
                &op.call,
                params,
                actor.clone(),
                code_sink.clone(),
            )
            .await
            .map_err(|e| BatchError::QueryError {
                alias: String::new(),
                message: e.to_string(),
                code: code_sink.take(),
            })?;

        // Map QueryValue -> QueryResult with `value` field.
        Ok(QueryResult::with_value(qv))
    }
}
