use std::sync::Arc;

use async_trait::async_trait;

use crate::access::Actor;
use crate::engine::function::{DbGateway, NetGateway};
use crate::engine::query::batch::{BatchError, BatchOp, BatchRequest, QueryEntry, ResultEncoding};
use crate::engine::query::read::{ReadQuery, Temporal};
use crate::engine::query::write::InsertOp;
use crate::engine::query::TableRef;
use crate::engine::query::{Filter, FilterValue};
use crate::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::super::ShamirDb;

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
            visibility_wait: None,
            durability: None,
            index_visibility: None,
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
            visibility_wait: None,
            durability: None,
            index_visibility: None,
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
            visibility_wait: None,
            durability: None,
            index_visibility: None,
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
        let net_grants = self.function_meta(fn_name).map(|m| m.net_grants);
        let effective = match net_grants {
            // No catalogue entry (builtin, not a user function going
            // through `CreateFunctionOptions`): fall back to the full
            // DB-wide allowlist — a different code path from "explicit
            // empty grants" below.
            None => self.net_allowlist.to_vec(),
            // Explicitly empty grants: NO egress at all (task #609).
            Some(grants) if grants.is_empty() => Vec::new(),
            // Non-empty grants: intersect with the DB-wide allowlist so the
            // function is scoped to the OVERLAP, never beyond the DB ceiling.
            Some(grants) => self
                .net_allowlist
                .iter()
                .filter(|host| grants.contains(host))
                .cloned()
                .collect(),
        };
        Arc::new(crate::shamir_db::CurlNetGateway::new(effective))
    }
}
