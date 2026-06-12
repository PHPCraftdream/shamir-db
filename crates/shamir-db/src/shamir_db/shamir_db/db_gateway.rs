use async_trait::async_trait;
use serde_json::json;

use crate::access::Actor;
use crate::engine::function::DbGateway;
use crate::engine::query::batch::{BatchError, BatchOp, BatchRequest, QueryEntry};
use crate::engine::query::read::{ReadQuery, Temporal};
use crate::engine::query::write::InsertOp;
use crate::engine::query::TableRef;
use crate::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::ShamirDb;

// ── FacadeDbGateway ──────────────────────────────────────────────────

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
    /// Convert a `QueryValue` key into a JSON filter suitable for a `ReadQuery`.
    ///
    /// Key convention:
    /// - `QueryValue::Map` → conjunction of `Eq` filters on each entry.
    /// - Scalar `QueryValue` (e.g. `Int`, `Str`) → `Eq` on the `"id"` field.
    fn key_to_filter(key: &QueryValue) -> serde_json::Value {
        match key {
            QueryValue::Map(entries) => {
                if entries.is_empty() {
                    return json!(null);
                }
                let filters: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|(field, val)| {
                        let json_val = serde_json::to_value(val).unwrap_or(json!(null));
                        json!({
                            "op": "eq",
                            "field": [field],
                            "value": json_val
                        })
                    })
                    .collect();
                if filters.len() == 1 {
                    return filters.into_iter().next().unwrap_or(json!(null));
                }
                json!({
                    "op": "and",
                    "filters": filters
                })
            }
            other => {
                let json_val = serde_json::to_value(other).unwrap_or(json!(null));
                json!({
                    "op": "eq",
                    "field": ["id"],
                    "value": json_val
                })
            }
        }
    }

    fn batch_err_to_string(e: BatchError) -> String {
        format!("{e:?}")
    }
}

#[async_trait]
impl DbGateway for FacadeDbGateway {
    async fn get(
        &self,
        repo: &str,
        table: &str,
        key: QueryValue,
    ) -> Result<Option<QueryValue>, String> {
        let filter = Self::key_to_filter(&key);
        let table_ref = if repo == "main" {
            TableRef::new(table)
        } else {
            TableRef::with_repo(repo, table)
        };

        let where_clause = if filter.is_null() {
            None
        } else {
            Some(
                serde_json::from_value(filter)
                    .map_err(|e| format!("get: filter parse error: {e}"))?,
            )
        };

        let read_query = ReadQuery {
            from: table_ref,
            select: crate::engine::query::read::Select::all(),
            r#where: where_clause,
            group_by: None,
            order_by: None,
            pagination: crate::engine::query::read::Pagination::None,
            count_total: false,
            temporal: Temporal::Latest,
            with_version: false,
        };

        let mut queries = new_map();
        queries.insert(
            "r".to_string(),
            QueryEntry {
                op: BatchOp::Read(read_query),
                return_result: true,
                after: Vec::new(),
            },
        );
        let req = BatchRequest {
            id: json!("db_get"),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: false,
            return_only: Some(vec!["r".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
        };

        let resp = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &req)
            .await
            .map_err(Self::batch_err_to_string)?;

        let result = match resp.results.get("r") {
            Some(r) => r,
            None => return Ok(None),
        };

        match result.records.first() {
            Some(rec) => {
                let qv = serde_json::from_value(rec.clone())
                    .map_err(|e| format!("get: record decode error: {e}"))?;
                Ok(Some(qv))
            }
            None => Ok(None),
        }
    }

    async fn insert(&self, repo: &str, table: &str, doc: QueryValue) -> Result<QueryValue, String> {
        let table_ref = if repo == "main" {
            TableRef::new(table)
        } else {
            TableRef::with_repo(repo, table)
        };

        let json_val =
            serde_json::to_value(&doc).map_err(|e| format!("insert: doc encode error: {e}"))?;

        let insert_op = InsertOp {
            insert_into: table_ref,
            values: vec![json_val.into()],
        };

        let mut queries = new_map();
        queries.insert(
            "i".to_string(),
            QueryEntry {
                op: BatchOp::Insert(insert_op),
                return_result: true,
                after: Vec::new(),
            },
        );
        let req = BatchRequest {
            id: json!("db_insert"),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: false,
            return_only: Some(vec!["i".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
        };

        let resp = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &req)
            .await
            .map_err(Self::batch_err_to_string)?;

        let result = match resp.results.get("i") {
            Some(r) => r,
            None => return Err("insert: no result returned".to_string()),
        };

        match result.records.first() {
            Some(rec) => {
                let qv = serde_json::from_value(rec.clone())
                    .map_err(|e| format!("insert: record decode error: {e}"))?;
                Ok(qv)
            }
            None => Err("insert: empty result".to_string()),
        }
    }

    async fn query(
        &self,
        repo: &str,
        table: &str,
        filter: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, String> {
        let table_ref = if repo == "main" {
            TableRef::new(table)
        } else {
            TableRef::with_repo(repo, table)
        };

        let where_clause = match filter {
            Some(f) => {
                let json_filter = Self::key_to_filter(&f);
                if json_filter.is_null() {
                    None
                } else {
                    Some(
                        serde_json::from_value(json_filter)
                            .map_err(|e| format!("query: filter parse error: {e}"))?,
                    )
                }
            }
            None => None,
        };

        let read_query = ReadQuery {
            from: table_ref,
            select: crate::engine::query::read::Select::all(),
            r#where: where_clause,
            group_by: None,
            order_by: None,
            pagination: crate::engine::query::read::Pagination::None,
            count_total: false,
            temporal: Temporal::Latest,
            with_version: false,
        };

        let mut queries = new_map();
        queries.insert(
            "q".to_string(),
            QueryEntry {
                op: BatchOp::Read(read_query),
                return_result: true,
                after: Vec::new(),
            },
        );
        let req = BatchRequest {
            id: json!("db_query"),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: false,
            return_only: Some(vec!["q".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
        };

        let resp = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &req)
            .await
            .map_err(Self::batch_err_to_string)?;

        let result = match resp.results.get("q") {
            Some(r) => r,
            None => return Ok(Vec::new()),
        };

        result
            .records
            .iter()
            .map(|rec| {
                serde_json::from_value(rec.clone())
                    .map_err(|e| format!("query: record decode error: {e}"))
            })
            .collect()
    }

    async fn execute(&self, request: &[u8]) -> Result<Vec<u8>, String> {
        let req: BatchRequest = rmp_serde::from_slice(request)
            .map_err(|e| format!("execute: request decode error: {e}"))?;
        let resp = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &req)
            .await
            .map_err(Self::batch_err_to_string)?;
        rmp_serde::to_vec_named(&resp).map_err(|e| format!("execute: response encode error: {e}"))
    }
}
