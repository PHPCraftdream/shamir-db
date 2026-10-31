use crate::engine::query::batch::{BatchError, BatchLimits, BatchOp, BatchRequest, ResultEncoding};
use crate::engine::query::read::ReadQuery;
use crate::engine::query::write::InsertOp;
use crate::engine::query::{Filter, FilterValue, TableRef};
use crate::types::value::QueryValue;
use crate::{
    access::Actor,
    engine::function::{DbGateway, DbGatewayError, NetGateway},
    shamir_db::shamir_db::ShamirDb,
};
use async_trait::async_trait;
use std::sync::Arc;

/// [`DbGateway`] implementation that routes through [`ShamirDb::execute`].
///
/// Each method builds a single-op [`BatchRequest`] and submits it via
/// `execute`, which commits independently (autocommit-per-op).
///
/// # Re-entrancy note
///
/// For this slice functions are invoked standalone (not from within an
/// `execute` call), so routing back through `execute` is safe. When
/// functions are later invoked *as batch ops*, the gateway must inherit
/// the batch's transaction instead of opening a new `execute` — otherwise
/// it would deadlock on the batch planner.
pub(super) struct FacadeDbGateway {
    pub(super) shamir: ShamirDb,
    pub(super) db_name: String,
    /// Effective actor of the invoking function (caller, or function owner
    /// under setuid).  The gateway runs the function's DB access AS this
    /// actor so per-table ACLs apply — NOT as System.
    pub(super) actor: Actor,
}

impl FacadeDbGateway {
    /// Convert a [`QueryValue`] scalar into a [`FilterValue`] for use in a
    /// `Filter::Eq` predicate. Unsupported values retain the field-specific
    /// error classification expected by the function gateway.
    fn qv_to_filter_value(val: &QueryValue, field: &str) -> Result<FilterValue, String> {
        match val {
            QueryValue::Null => Ok(FilterValue::Null),
            QueryValue::Bool(v) => Ok(FilterValue::Bool(*v)),
            QueryValue::Int(v) => Ok(FilterValue::Int(*v)),
            QueryValue::F64(v) => Ok(FilterValue::Float(*v)),
            QueryValue::Str(v) => Ok(FilterValue::String(v.clone())),
            QueryValue::Bin(v) => Ok(FilterValue::Binary(v.clone())),
            QueryValue::List(_) => Err(format!("unsupported key value List for field {field}")),
            QueryValue::Map(_) => Err(format!("unsupported key value Map for field {field}")),
            QueryValue::Set(_) => Err(format!("unsupported key value Set for field {field}")),
            QueryValue::Dec(_) => Err(format!("unsupported key value Dec for field {field}")),
            QueryValue::Big(_) => Err(format!("unsupported key value Big for field {field}")),
        }
    }

    /// Convert a `QueryValue` key into a [`Filter`] suitable for a `ReadQuery`.
    ///
    /// Key convention:
    /// - `QueryValue::Map` → conjunction of `Eq` filters on each entry.
    /// - Scalar `QueryValue` (e.g. `Int`, `Str`) → `Eq` on the `"id"` field.
    ///
    /// Returns `None` for an empty map (match-all / no filter).
    /// WASM-bridge exception: guest msgpack `Value` key -> typed `Filter`; use
    /// `Db::execute` (feature `query-builder`) for builder queries.
    fn key_to_filter(key: &QueryValue) -> Result<Option<Filter>, String> {
        match key {
            QueryValue::Map(entries) => {
                if entries.is_empty() {
                    return Ok(None);
                }
                let filters: Vec<_> = entries
                    .iter()
                    .map(|(field, value)| {
                        Ok(shamir_query_builder::filter::eq(
                            vec![field.clone()],
                            Self::qv_to_filter_value(value, field)?,
                        ))
                    })
                    .collect::<Result<_, String>>()?;
                Ok(Some(if filters.len() == 1 {
                    filters.into_iter().next().expect("one filter")
                } else {
                    shamir_query_builder::filter::and(filters)
                }))
            }
            value => Ok(Some(shamir_query_builder::filter::eq(
                "id",
                Self::qv_to_filter_value(value, "id")?,
            ))),
        }
    }

    fn table_ref(repo: &str, table: &str) -> TableRef {
        if repo == "main" {
            TableRef::new(table)
        } else {
            TableRef::with_repo(repo, table)
        }
    }

    /// Construct a read through the query builder, preserving today's defaults.
    fn read_query(repo: &str, table: &str, filter: Option<Filter>) -> ReadQuery {
        let query = if repo == "main" {
            shamir_query_builder::Query::from(table)
        } else {
            shamir_query_builder::Query::with_repo(repo, table)
        };
        let query = match filter {
            Some(filter) => query.where_(filter),
            None => query,
        };
        query.build()
    }

    /// Assemble the one-operation request with EVERY batch field set to
    /// today's value explicitly (RV-DB-C2DCAC96): via builder setters where
    /// they exist, else by overriding the built [`BatchRequest`] fields.
    fn request(alias: &str, id: &str, op: BatchOp) -> BatchRequest {
        let mut batch = shamir_query_builder::batch::Batch::new();
        batch.id(QueryValue::Str(id.to_string()));
        batch.return_only([alias]);
        match op {
            BatchOp::Read(query) => {
                batch.query(alias, query);
            }
            BatchOp::Insert(insert) => {
                batch.insert(alias, insert);
            }
            _ => unreachable!("gateway request operation kind is fixed"),
        }
        let mut request = batch.build();
        // The builder has no "unset to None" setters; these four plus the
        // transaction flags default to None/false in Batch::new() and must
        // stay exactly that, so they are pinned explicitly here.
        request.name = None;
        request.transactional = false;
        request.isolation = None;
        request.visibility_wait = None;
        request.durability = None;
        request.index_visibility = None;
        request.return_all = false;
        request.limits = BatchLimits::default();
        request.interner_epochs = Default::default();
        request.result_encoding = ResultEncoding::default();
        request
    }
}

/// Classify the gateway failure while retaining the original batch error.
pub(crate) fn classify_batch_error(e: BatchError) -> DbGatewayError {
    match &e {
        BatchError::QueryError {
            message,
            code: Some(code),
            ..
        } if code == "not_found" => DbGatewayError::NotFound {
            message: message.clone(),
            source: Box::new(e),
        },
        BatchError::QueryError {
            message,
            code: Some(code),
            ..
        } if code == "access_denied" => DbGatewayError::Permission {
            message: message.clone(),
            source: Box::new(e),
        },
        _ => DbGatewayError::ExecutionSource {
            message: e.to_string(),
            source: Box::new(e),
        },
    }
}

#[async_trait]
impl DbGateway for FacadeDbGateway {
    async fn get(
        &self,
        repo: &str,
        table: &str,
        key: QueryValue,
    ) -> Result<Option<QueryValue>, DbGatewayError> {
        let filter = Self::key_to_filter(&key).map_err(DbGatewayError::Execution)?;
        let query = Self::read_query(repo, table, filter);
        let request = Self::request("r", "db_get", BatchOp::Read(query));
        let response = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &request)
            .await
            .map_err(classify_batch_error)?;
        Ok(response
            .results
            .get("r")
            .and_then(|r| r.records.first())
            .map(|r| r.as_value().into_owned()))
    }

    async fn insert(
        &self,
        repo: &str,
        table: &str,
        doc: QueryValue,
    ) -> Result<QueryValue, DbGatewayError> {
        let insert = InsertOp {
            insert_into: Self::table_ref(repo, table),
            values: vec![doc],
            records_idmsgpack: Vec::new(),
            select: None,
        };
        let request = Self::request("i", "db_insert", BatchOp::Insert(insert));
        let response = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &request)
            .await
            .map_err(classify_batch_error)?;
        let result = match response.results.get("i") {
            Some(r) => r,
            None => {
                return Err(DbGatewayError::Execution(
                    "insert: no result returned".to_string(),
                ))
            }
        };
        match result.records.first() {
            Some(rec) => Ok(rec.as_value().into_owned()),
            None => Err(DbGatewayError::Execution(
                "insert: empty result".to_string(),
            )),
        }
    }

    async fn query(
        &self,
        repo: &str,
        table: &str,
        filter: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, DbGatewayError> {
        let filter = filter
            .as_ref()
            .map(Self::key_to_filter)
            .transpose()
            .map_err(DbGatewayError::Execution)?
            .flatten();
        let query = Self::read_query(repo, table, filter);
        let request = Self::request("q", "db_query", BatchOp::Read(query));
        let response = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &request)
            .await
            .map_err(classify_batch_error)?;
        Ok(response
            .results
            .get("q")
            .map(|r| {
                r.records
                    .iter()
                    .map(|rec| rec.as_value().into_owned())
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn execute(&self, request: &[u8]) -> Result<Vec<u8>, DbGatewayError> {
        let req: BatchRequest =
            rmp_serde::from_slice(request).map_err(|e| DbGatewayError::ExecutionSource {
                message: format!("execute: request decode error: {e}"),
                source: Box::new(e),
            })?;
        let response = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &req)
            .await
            .map_err(classify_batch_error)?;
        rmp_serde::to_vec_named(&response).map_err(|e| DbGatewayError::ExecutionSource {
            message: format!("execute: response encode error: {e}"),
            source: Box::new(e),
        })
    }
}

impl ShamirDb {
    /// Build a [`NetGateway`] scoped to `fn_name`'s effective egress reach:
    /// the function's `net_grants` INTERSECTED with the DB-wide
    /// `net_allowlist` (a function can never exceed the DB's own ceiling).
    ///
    /// Always returns a gateway so that allowlist-denial is a catchable
    /// runtime error, not a "no net gateway" trap.
    ///
    /// # Empty `net_grants` semantics
    ///
    /// Task #609: empty `net_grants` now means NO egress, matching
    /// `secret_grants`'s restrictive-by-default precedent. No
    /// backward-compatibility constraint applies (pre-release). A
    /// non-empty `net_grants` list narrows the function to LESS than the DB
    /// default — there is no way to grant a function MORE than the DB
    /// ceiling.
    ///
    /// # Intersection is literal-string, not pattern-aware (known limitation)
    ///
    /// The intersection below matches `net_grants` entries against
    /// `net_allowlist` entries by EXACT STRING equality, not by resolving
    /// either side against an actual host the way `check_host_allowed`'s
    /// `glob_matches` does at fetch time. Concretely: if the DB allowlist
    /// has the wildcard pattern `"*.example.com"` and a function's
    /// `net_grants` names a concrete host like `"api.example.com"`, the two
    /// strings don't match, so the intersection is EMPTY and the function is
    /// denied ALL egress — even though `api.example.com` is both covered by
    /// the DB's wildcard and clearly what the grant intended. This fails
    /// CLOSED (never grants more than intended), so it's not a security
    /// hole, but it means an operator narrowing a wildcard DB allowlist to a
    /// concrete per-function host must currently spell `net_grants` with
    /// the exact same pattern string as it appears in `net_allowlist` — a
    /// real usability gap, not fixed by this task (see task #544's closing
    /// report / a follow-up would need to intersect via pattern-subsumption
    /// rather than string equality).
    pub(crate) fn build_net_gateway(&self, fn_name: &str) -> Arc<dyn NetGateway> {
        let meta = self.function_meta.get(fn_name);
        self.net_allowlist
            .gateway(meta.as_ref().map(|m| m.net_grants.as_slice()))
    }
}

#[cfg(test)]
#[path = "../../tests/gateway/db_gateway_request_tests.rs"]
mod db_gateway_request_tests;
