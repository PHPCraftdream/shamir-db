use async_trait::async_trait;

use crate::access::Actor;
use crate::engine::function::DbGateway;
use crate::engine::query::batch::{BatchError, BatchOp, BatchRequest, QueryEntry, ResultEncoding};
use crate::engine::query::read::{ReadQuery, Temporal};
use crate::engine::query::write::InsertOp;
use crate::engine::query::TableRef;
use crate::engine::query::{Filter, FilterValue};
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
    /// Convert a [`QueryValue`] scalar into a [`FilterValue`] for use in a
    /// `Filter::Eq` predicate.
    fn qv_to_filter_value(val: &QueryValue) -> FilterValue {
        match val {
            QueryValue::Null => FilterValue::Null,
            QueryValue::Bool(b) => FilterValue::Bool(*b),
            QueryValue::Int(i) => FilterValue::Int(*i),
            QueryValue::F64(f) => FilterValue::Float(*f),
            QueryValue::Str(s) => FilterValue::String(s.clone()),
            QueryValue::Bin(b) => FilterValue::Binary(b.clone()),
            // For composite or array values fall back to Null — these are not
            // expected as key components in the gateway's key_to_filter path.
            _ => FilterValue::Null,
        }
    }

    /// Convert a `QueryValue` key into a [`Filter`] suitable for a `ReadQuery`.
    ///
    /// Key convention:
    /// - `QueryValue::Map` → conjunction of `Eq` filters on each entry.
    /// - Scalar `QueryValue` (e.g. `Int`, `Str`) → `Eq` on the `"id"` field.
    ///
    /// Returns `None` for an empty map (match-all / no filter).
    fn key_to_filter(key: &QueryValue) -> Option<Filter> {
        match key {
            QueryValue::Map(entries) => {
                if entries.is_empty() {
                    return None;
                }
                let mut filters: Vec<Filter> = entries
                    .iter()
                    .map(|(field, val)| Filter::Eq {
                        field: vec![field.clone()],
                        value: Self::qv_to_filter_value(val),
                    })
                    .collect();
                if filters.len() == 1 {
                    return Some(filters.remove(0));
                }
                Some(Filter::And { filters })
            }
            other => Some(Filter::Eq {
                field: vec!["id".to_string()],
                value: Self::qv_to_filter_value(other),
            }),
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
        let where_clause = Self::key_to_filter(&key);
        let table_ref = if repo == "main" {
            TableRef::new(table)
        } else {
            TableRef::with_repo(repo, table)
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
            explain: false,
        };

        let mut queries = new_map();
        queries.insert(
            "r".to_string(),
            QueryEntry {
                op: BatchOp::Read(read_query),
                return_result: true,
                after: Vec::new(),
                when: None,
            },
        );
        let req = BatchRequest {
            id: QueryValue::Str("db_get".into()),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: false,
            return_only: Some(vec!["r".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
            interner_epochs: Default::default(),
            result_encoding: ResultEncoding::default(),
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
            Some(rec) => Ok(Some(rec.as_value().into_owned())),
            None => Ok(None),
        }
    }

    async fn insert(&self, repo: &str, table: &str, doc: QueryValue) -> Result<QueryValue, String> {
        let table_ref = if repo == "main" {
            TableRef::new(table)
        } else {
            TableRef::with_repo(repo, table)
        };

        let insert_op = InsertOp {
            insert_into: table_ref,
            values: vec![doc],
            records_idmsgpack: Vec::new(),
            select: None,
        };

        let mut queries = new_map();
        queries.insert(
            "i".to_string(),
            QueryEntry {
                op: BatchOp::Insert(insert_op),
                return_result: true,
                after: Vec::new(),
                when: None,
            },
        );
        let req = BatchRequest {
            id: QueryValue::Str("db_insert".into()),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: false,
            return_only: Some(vec!["i".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
            interner_epochs: Default::default(),
            result_encoding: ResultEncoding::default(),
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
            Some(rec) => Ok(rec.as_value().into_owned()),
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

        let where_clause = filter.as_ref().and_then(Self::key_to_filter);

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
            explain: false,
        };

        let mut queries = new_map();
        queries.insert(
            "q".to_string(),
            QueryEntry {
                op: BatchOp::Read(read_query),
                return_result: true,
                after: Vec::new(),
                when: None,
            },
        );
        let req = BatchRequest {
            id: QueryValue::Str("db_query".into()),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: false,
            return_only: Some(vec!["q".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
            interner_epochs: Default::default(),
            result_encoding: ResultEncoding::default(),
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

        Ok(result
            .records
            .iter()
            .map(|rec| rec.as_value().into_owned())
            .collect())
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
