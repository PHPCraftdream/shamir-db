//! `RequestHandler` bridge between `shamir-connect` post-handshake dispatch
//! and the `shamir-db` query layer.
//!
//! After SCRAM auth, every request comes in as `RequestEnvelope { session_id,
//! request_id, req: Vec<u8> }`. The transport calls
//! `dispatch_request_view(view, store, lookup_tickets_invalid, handler)`,
//! which runs the §7.5 validity check then invokes
//! [`RequestHandler::handle`]. This module implements that trait against the
//! shared `ShamirDb` instance.
//!
//! # Wire format (msgpack, `rmp_serde::to_vec_named` / `from_slice`)
//!
//! - [`DbRequest`] — tagged enum of supported ops (`ping`, `get`, `set`,
//!   `delete`, `list_tables`).
//! - [`DbResponse`] — tagged enum of replies (`pong`, `ok`, `value`, `tables`,
//!   `error`).
//!
//! # Error semantics
//!
//! `RequestHandler::handle` returns `Err(reason)` **only** for
//! SHAMIR-protocol-level failures (mapped to `ErrorEnvelope`). DB-layer
//! failures (db not found, key missing, type mismatch) are returned inside
//! `Ok(bytes)` carrying a [`DbResponse::Error`] payload.
//!
//! Currently `Err` is returned only for malformed wire input (`from_slice`
//! decode failure of [`DbRequest`]).
//!
//! # Async bridge
//!
//! `RequestHandler::handle` is sync, but the ShamirDb query API is async.
//! We bridge with `tokio::task::block_in_place` + `Handle::current().block_on`
//! so the future is driven on the current Tokio worker without spawning a
//! second runtime. This requires a **multi-thread** Tokio runtime (which
//! every connection task runs on in production via `tokio::spawn`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::Session;

use shamir_db::db::query::filter::{Filter, FilterValue, FilterContext};
use shamir_db::db::query::read::ReadQuery;
use shamir_db::db::query::write::{DeleteOp, SetOp};
use shamir_db::db::query::TableRef;
use shamir_db::db::ShamirDb;

// --------------------------------------------------------------------------
// Wire schema
// --------------------------------------------------------------------------

/// Application-layer DB request (msgpack-encoded payload of
/// `RequestEnvelope.req`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DbRequest {
    /// Health check.
    Ping,
    /// Read a single record by key.
    Get {
        db: String,
        repo: String,
        table: String,
        key: serde_json::Value,
    },
    /// Upsert a record by key.
    Set {
        db: String,
        repo: String,
        table: String,
        key: serde_json::Value,
        value: serde_json::Value,
    },
    /// Delete records matching the key.
    Delete {
        db: String,
        repo: String,
        table: String,
        key: serde_json::Value,
    },
    /// List table names within a repo.
    ListTables { db: String, repo: String },
}

/// Application-layer DB response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DbResponse {
    /// Reply to `Ping`.
    Pong,
    /// Generic success with no payload (e.g. successful `Set` / `Delete`).
    Ok,
    /// `Get` hit — single record.
    Value { value: serde_json::Value },
    /// `ListTables` reply.
    Tables { names: Vec<String> },
    /// DB-layer error (not a protocol error). Caller still sees an
    /// `ResponseEnvelope`, just with this payload.
    Error { message: String },
}

impl DbResponse {
    /// Build a `DbResponse::Error { message: msg }`.
    fn error(msg: impl Into<String>) -> Self {
        Self::Error { message: msg.into() }
    }
}

// --------------------------------------------------------------------------
// Handler
// --------------------------------------------------------------------------

/// Bridge handler — dispatches decoded [`DbRequest`] against a shared
/// [`ShamirDb`] instance.
///
/// # Permissions (v1)
///
/// First-cut policy: any authenticated session may read or write. Future
/// hardening will gate writes on `session.permissions.read().is_superuser`
/// or membership in a `read_write` role (spec §12).
#[derive(Clone)]
pub struct ShamirDbHandler {
    db: Arc<ShamirDb>,
}

impl ShamirDbHandler {
    /// Construct a handler over a shared `ShamirDb`.
    pub fn new(db: Arc<ShamirDb>) -> Self {
        Self { db }
    }

    /// Reference to the underlying `ShamirDb` (for tests / admin glue).
    pub fn db(&self) -> &Arc<ShamirDb> {
        &self.db
    }
}

impl RequestHandler for ShamirDbHandler {
    fn handle(&self, _session: &Session, req: &[u8]) -> std::result::Result<Vec<u8>, String> {
        // Decode wire request. Failure → protocol-level error (Err →
        // ErrorEnvelope on the wire).
        let request: DbRequest = rmp_serde::from_slice(req)
            .map_err(|e| format!("invalid_request: {}", e))?;

        // Run the async dispatch on the current Tokio worker.
        let response = run_blocking(self.dispatch(request));

        // Encode the response. Encoding shouldn't fail for our schema, but
        // surface failures as protocol-level errors to be safe.
        rmp_serde::to_vec_named(&response).map_err(|e| format!("encode_error: {}", e))
    }
}

impl ShamirDbHandler {
    async fn dispatch(&self, req: DbRequest) -> DbResponse {
        match req {
            DbRequest::Ping => DbResponse::Pong,
            DbRequest::Get { db, repo, table, key } => self.do_get(&db, &repo, &table, key).await,
            DbRequest::Set { db, repo, table, key, value } => {
                self.do_set(&db, &repo, &table, key, value).await
            }
            DbRequest::Delete { db, repo, table, key } => {
                self.do_delete(&db, &repo, &table, key).await
            }
            DbRequest::ListTables { db, repo } => self.do_list_tables(&db, &repo).await,
        }
    }

    async fn do_get(
        &self,
        db_name: &str,
        repo: &str,
        table: &str,
        key: serde_json::Value,
    ) -> DbResponse {
        let filter = match key_to_filter(&key) {
            Ok(f) => f,
            Err(e) => return DbResponse::error(e),
        };

        let table_mgr = match self.db.get_table(db_name, repo, table).await {
            Ok(t) => t,
            Err(e) => return DbResponse::error(short_err(e)),
        };

        let interner = match table_mgr.interner().get().await {
            Ok(i) => i,
            Err(e) => return DbResponse::error(short_err(e)),
        };
        let refs = shamir_db::types::common::new_map();
        let ctx = FilterContext::new(interner, &refs);
        let query = ReadQuery::new(table).filter(filter);

        match table_mgr.read(&query, &ctx).await {
            Ok(result) => {
                if let Some(record) = result.records.into_iter().next() {
                    DbResponse::Value { value: record }
                } else {
                    DbResponse::error("not_found")
                }
            }
            Err(e) => DbResponse::error(short_err(e)),
        }
    }

    async fn do_set(
        &self,
        db_name: &str,
        repo: &str,
        table: &str,
        key: serde_json::Value,
        value: serde_json::Value,
    ) -> DbResponse {
        if !key.is_object() {
            return DbResponse::error("invalid_key: must be a JSON object");
        }

        let table_mgr = match self.db.get_table(db_name, repo, table).await {
            Ok(t) => t,
            Err(e) => return DbResponse::error(short_err(e)),
        };

        let op = SetOp {
            set: TableRef::with_repo(repo, table),
            key,
            value,
        };

        match table_mgr.execute_set(&op).await {
            Ok(_) => {
                // Persist any newly interned keys (mirrors batch executor).
                let _ = table_mgr.interner().persist().await;
                DbResponse::Ok
            }
            Err(e) => DbResponse::error(short_err(e)),
        }
    }

    async fn do_delete(
        &self,
        db_name: &str,
        repo: &str,
        table: &str,
        key: serde_json::Value,
    ) -> DbResponse {
        let filter = match key_to_filter(&key) {
            Ok(f) => f,
            Err(e) => return DbResponse::error(e),
        };

        let table_mgr = match self.db.get_table(db_name, repo, table).await {
            Ok(t) => t,
            Err(e) => return DbResponse::error(short_err(e)),
        };

        let interner = match table_mgr.interner().get().await {
            Ok(i) => i,
            Err(e) => return DbResponse::error(short_err(e)),
        };
        let refs = shamir_db::types::common::new_map();
        let ctx = FilterContext::new(interner, &refs);
        let op = DeleteOp {
            delete_from: TableRef::with_repo(repo, table),
            where_clause: filter,
        };

        match table_mgr.execute_delete(&op, &ctx).await {
            Ok(_) => DbResponse::Ok,
            Err(e) => DbResponse::error(short_err(e)),
        }
    }

    async fn do_list_tables(&self, db_name: &str, repo: &str) -> DbResponse {
        let db = match self.db.get_db(db_name) {
            Some(d) => d,
            None => return DbResponse::error("db_not_found"),
        };
        match db.list_tables(repo) {
            Ok(names) => DbResponse::Tables { names },
            Err(e) => DbResponse::error(short_err(e)),
        }
    }
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

/// Best-effort key → filter conversion. Accepts a JSON object whose values
/// are scalars (null/bool/int/float/string). Each entry is folded into a
/// conjunction of `Filter::Eq`. Empty object → error.
fn key_to_filter(key: &serde_json::Value) -> Result<Filter, String> {
    let obj = key.as_object().ok_or_else(|| {
        "invalid_key: must be a JSON object".to_string()
    })?;
    if obj.is_empty() {
        return Err("invalid_key: empty key object".to_string());
    }

    let mut clauses = Vec::with_capacity(obj.len());
    for (field, val) in obj {
        let fv = json_to_filter_value(val)?;
        clauses.push(Filter::Eq {
            field: vec![field.clone()],
            value: fv,
        });
    }

    Ok(if clauses.len() == 1 {
        clauses.into_iter().next().unwrap()
    } else {
        Filter::And { filters: clauses }
    })
}

/// Convert a JSON scalar to a [`FilterValue`]. Non-scalars are rejected.
fn json_to_filter_value(v: &serde_json::Value) -> Result<FilterValue, String> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => FilterValue::Null,
        J::Bool(b) => FilterValue::Bool(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                FilterValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                FilterValue::Float(f)
            } else {
                return Err("invalid_key: unsupported numeric type".to_string());
            }
        }
        J::String(s) => FilterValue::String(s.clone()),
        J::Array(_) | J::Object(_) => {
            return Err("invalid_key: scalar values only in key fields".to_string())
        }
    })
}

/// Bridge an async future to a sync caller running inside a Tokio worker.
///
/// `block_in_place` lets us call `block_on` without panicking with
/// "Cannot start a runtime from within a runtime". This requires a
/// multi-thread runtime — single-thread (`current_thread`) flavor will
/// also panic. Tests therefore use `#[tokio::test(flavor = "multi_thread")]`
/// and the production server uses the default multi-thread runtime.
fn run_blocking<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// Turn a `DbError`-like display into a short, single-line error message.
fn short_err<E: std::fmt::Display>(e: E) -> String {
    let s = e.to_string();
    // Take first line; cap to 200 chars for safety.
    let line = s.lines().next().unwrap_or("error");
    if line.len() > 200 { line[..200].to_string() } else { line.to_string() }
}
