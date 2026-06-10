//! Shared helper functions used across admin executor sub-modules.

use crate::query::batch::BatchError;
use crate::query::read::{QueryResult, QueryStats};
use crate::query::FilterValue;
use crate::types::common::TMap;
use crate::types::value::QueryValue;

/// Construct a `QueryResult` for a successful admin operation.
pub(super) fn admin_result(data: serde_json::Value) -> QueryResult {
    QueryResult {
        records: vec![data],
        stats: Some(QueryStats {
            index_used: None,
            records_scanned: 0,
            records_returned: 1,
            execution_time_us: 0,
        }),
        pagination: None,
        value: None,
    }
}

/// Rejects path-traversal characters in database and repository names.
///
/// Only `[A-Za-z0-9_-]` is allowed — no `/`, `\`, `:`, `.`, or any
/// non-ASCII byte. Empty strings are also rejected.
pub(super) fn validate_name_component(s: &str, label: &str) -> Result<(), BatchError> {
    if s.is_empty() {
        return Err(BatchError::QueryError {
            alias: String::new(),
            message: format!("{} must not be empty", label),
            code: None,
        });
    }
    if s == "." || s == ".." {
        return Err(BatchError::QueryError {
            alias: String::new(),
            message: format!("{} must not be '.' or '..'", label),
            code: None,
        });
    }
    for ch in s.chars() {
        if !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-' {
            return Err(BatchError::QueryError {
                alias: String::new(),
                message: format!(
                    "{} contains disallowed character '{}': \
                     only [A-Za-z0-9_-] are permitted",
                    label, ch
                ),
                code: None,
            });
        }
    }
    Ok(())
}

/// Map the wire DTO into the storage struct without dragging the
/// storage crate's serde-compatible-by-coincidence layout into
/// the API contract — the two types are intentionally distinct.
pub(super) fn storage_from_dto(
    dto: &crate::query::admin::BufferConfigDto,
) -> crate::storage::storage_membuffer::MemBufferConfig {
    crate::storage::storage_membuffer::MemBufferConfig {
        max_bytes: dto.max_bytes,
        max_entries: dto.max_entries,
        ttl_ms: dto.ttl_ms,
        flush_interval_ms: dto.flush_interval_ms,
        flush_batch_size: dto.flush_batch_size,
    }
}

pub(super) fn dto_from_storage(
    cfg: &crate::storage::storage_membuffer::MemBufferConfig,
) -> crate::query::admin::BufferConfigDto {
    crate::query::admin::BufferConfigDto {
        max_bytes: cfg.max_bytes,
        max_entries: cfg.max_entries,
        ttl_ms: cfg.ttl_ms,
        flush_interval_ms: cfg.flush_interval_ms,
        flush_batch_size: cfg.flush_batch_size,
    }
}

/// Apply only the fields the patch actually set; leave the rest
/// alone. Double-option semantics for `ttl_ms`: `Some(None)` ↔
/// "clear TTL"; `Some(Some(v))` ↔ "set TTL"; `None` ↔ "untouched".
pub(super) fn apply_patch(
    cfg: &mut crate::storage::storage_membuffer::MemBufferConfig,
    patch: &crate::query::admin::BufferConfigPatch,
) {
    if let Some(v) = patch.max_bytes {
        cfg.max_bytes = v;
    }
    if let Some(v) = patch.max_entries {
        cfg.max_entries = v;
    }
    if let Some(v) = patch.ttl_ms {
        cfg.ttl_ms = v;
    }
    if let Some(v) = patch.flush_interval_ms {
        cfg.flush_interval_ms = v;
    }
    if let Some(v) = patch.flush_batch_size {
        cfg.flush_batch_size = v;
    }
}

/// Hash a plaintext password into an Argon2id PHC string for at-rest
/// storage in the `users` table. Salt is drawn from the OS CSPRNG
/// (`OsRng`) per a fresh 16-byte `SaltString`; params are the `argon2`
/// crate defaults (Argon2id, v0x13). Returns the self-describing PHC
/// string (`$argon2id$v=19$m=...$<salt>$<hash>`), which embeds the salt
/// and params so verification needs no side-channel state.
///
/// NOTE: this field is admin/RBAC metadata, not the live-auth
/// credential — wire login is SCRAM-Argon2id in `shamir-connect`. No
/// verify site reads `users.password_hash`, so hashing here is purely
/// defense-in-depth at rest.
pub(super) fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::Argon2;
    use rand::rngs::OsRng;

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// T3: look up a table's `MvccStore` and apply a retention policy.
///
/// `per_table_mvcc` is lazily populated on first `get_table()`, so we
/// force table instantiation first, then resolve the store by token.
/// If the table has no MvccStore entry (shouldn't happen for a real
/// table), the policy is skipped gracefully — the next instantiation
/// will pick up CurrentOnly (the `MvccStore::new` default).
pub(super) async fn apply_table_retention(
    shamir: &super::super::shamir_db::ShamirDb,
    db_name: &str,
    repo: &str,
    table: &str,
    policy: crate::engine::repo::MvccRetention,
) -> Result<(), BatchError> {
    let err = |msg: String| BatchError::QueryError {
        alias: String::new(),
        message: msg,
        code: None,
    };
    let db = shamir
        .get_db(db_name)
        .ok_or_else(|| err(format!("Database '{}' not found", db_name)))?;
    // Force the lazy MvccStore entry for this table.
    let _ = db
        .get_table(repo, table)
        .await
        .map_err(|e| err(e.to_string()))?;
    let repo_instance = db
        .get_repo(repo)
        .ok_or_else(|| err(format!("Repository '{}' not found", repo)))?;
    let token = crate::engine::table::table_token_for(table);
    if let Some(entry) = repo_instance.per_table_mvcc().get(&token) {
        entry
            .set_retention(policy)
            .map_err(|e| err(e.to_string()))?;
    }
    Ok(())
}

/// T4-purge: resolve the live `MvccStore` for `(db, repo, table)`.
///
/// Mirrors [`apply_table_retention`]'s lookup but RETURNS the store
/// (needed so the caller can read its clock and run the purge). Forces
/// table instantiation first (`per_table_mvcc` is lazily populated on
/// first `get_table`). Errors clearly on unknown db / repo / table or
/// a table with no MvccStore entry.
pub(super) async fn resolve_table_mvcc(
    shamir: &super::super::shamir_db::ShamirDb,
    db_name: &str,
    repo: &str,
    table: &str,
) -> Result<std::sync::Arc<shamir_tx::MvccStore>, BatchError> {
    let err = |msg: String| BatchError::QueryError {
        alias: String::new(),
        message: msg,
        code: None,
    };
    let db = shamir
        .get_db(db_name)
        .ok_or_else(|| err(format!("Database '{}' not found", db_name)))?;
    // Force the lazy MvccStore entry for this table.
    let _ = db
        .get_table(repo, table)
        .await
        .map_err(|e| err(e.to_string()))?;
    let repo_instance = db
        .get_repo(repo)
        .ok_or_else(|| err(format!("Repository '{}' not found", repo)))?;
    let token = crate::engine::table::table_token_for(table);
    repo_instance
        .per_table_mvcc()
        .get(&token)
        .map(|entry| std::sync::Arc::clone(&entry))
        .ok_or_else(|| {
            err(format!(
                "Table '{}.{}' has no MvccStore (History/Purge require an MVCC-backed table)",
                repo, table
            ))
        })
}

/// Convert a `FilterValue` literal to a `QueryValue`.
///
/// Literals (Null / Bool / Int / Float / String / Binary / Array) are mapped
/// directly. `$query` / `QueryRef` variants are resolved against
/// `resolved_refs` — the same value-first / records-second rules as the
/// filter evaluator (Phase 2). Other dynamic variants (`$ref`, `$fn`, `$expr`,
/// `$cond`) collapse to `Null` here; they are not meaningful as Call params.
pub(super) fn filter_value_to_query_value(
    fv: &FilterValue,
    resolved_refs: &TMap<String, QueryResult>,
) -> QueryValue {
    match fv {
        FilterValue::Null => QueryValue::Null,
        FilterValue::Bool(b) => QueryValue::Bool(*b),
        FilterValue::Int(i) => QueryValue::Int(*i),
        FilterValue::Float(f) => QueryValue::F64(*f),
        FilterValue::String(s) => QueryValue::Str(s.clone()),
        FilterValue::Binary(b) => QueryValue::Bin(b.clone()),
        FilterValue::Array(arr) => QueryValue::List(
            arr.iter()
                .map(|v| filter_value_to_query_value(v, resolved_refs))
                .collect(),
        ),
        FilterValue::QueryRef { alias, path } => {
            let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
            let Some(qr) = resolved_refs.get(key) else {
                return QueryValue::Null;
            };
            // Same value-first / records-second rule as the filter evaluator:
            // a Call result lives in `value`; a Read result lives in `records`.
            if let Some(value) = &qr.value {
                json_value_to_query_value(value, path.as_deref())
            } else if path.is_none() {
                // No path + Read result: synthesize from the records array.
                let arr: Vec<QueryValue> = qr
                    .records
                    .iter()
                    .map(|r| json_value_to_query_value(r, None))
                    .collect();
                QueryValue::List(arr)
            } else {
                // Indexed/field path into records. Only the `[n]` form is
                // meaningful without a record context; walk to the record
                // then serialise it.
                let path = path.as_deref().unwrap_or("");
                if let Some(rest) = path.strip_prefix('[') {
                    if let Some(end) = rest.find(']') {
                        if let Ok(idx) = rest[..end].parse::<usize>() {
                            if let Some(record) = qr.records.get(idx) {
                                let after = &rest[end + 1..];
                                if let Some(field_path) = after.strip_prefix('.') {
                                    if let Some(field_val) = record.get(field_path) {
                                        return json_value_to_query_value(field_val, None);
                                    }
                                    return QueryValue::Null;
                                }
                                return json_value_to_query_value(record, None);
                            }
                        }
                    }
                }
                QueryValue::Null
            }
        }
        // $ref / $fn / $expr / $cond — not meaningful as positional params.
        _ => QueryValue::Null,
    }
}

/// Convert a `serde_json::Value` (the wire representation used in
/// `QueryResult.value` / `QueryResult.records`) into a `QueryValue`, with
/// optional path navigation. Used to resolve `$query` refs inside Call
/// params.
pub(super) fn json_value_to_query_value(v: &serde_json::Value, path: Option<&str>) -> QueryValue {
    let Some(target) = navigate_json_value(v, path) else {
        return QueryValue::Null;
    };
    match target {
        serde_json::Value::Null => QueryValue::Null,
        serde_json::Value::Bool(b) => QueryValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                QueryValue::Int(i)
            } else {
                QueryValue::F64(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => QueryValue::Str(s.clone()),
        serde_json::Value::Array(arr) => QueryValue::List(
            arr.iter()
                .map(|v| json_value_to_query_value(v, None))
                .collect(),
        ),
        serde_json::Value::Object(map) => {
            let mut out = crate::types::common::new_map();
            for (k, vv) in map {
                out.insert(k.clone(), json_value_to_query_value(vv, None));
            }
            QueryValue::Map(out)
        }
    }
}

/// Walk a path like `.field`, `[0]`, `[0].name` through a `serde_json::Value`.
/// Mirrors `resolve_json_path` in the filter evaluator — duplicated here to
/// keep `shamir-db` independent of `shamir-engine::query::filter::eval`
/// (which is crate-private). Returns `None` on any miss / unsupported syntax.
fn navigate_json_value<'a>(
    mut cur: &'a serde_json::Value,
    path: Option<&str>,
) -> Option<&'a serde_json::Value> {
    let Some(path) = path else {
        return Some(cur);
    };
    let mut rest = path;
    while !rest.is_empty() {
        if let Some(after_dot) = rest.strip_prefix('.') {
            let end = after_dot.find(['.', '[']).unwrap_or(after_dot.len());
            cur = cur.get(&after_dot[..end])?;
            rest = &after_dot[end..];
        } else if rest.starts_with('[') {
            let bracket_end = rest.find(']')?;
            let idx: usize = rest[1..bracket_end].parse().ok()?;
            cur = cur.get(idx)?;
            rest = &rest[bracket_end + 1..];
        } else {
            return None;
        }
    }
    Some(cur)
}
