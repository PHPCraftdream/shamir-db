use crate::query::batch::BatchError;
use crate::shamir_db::ShamirDb;
use shamir_engine::repo::RepoWritePermit;

pub(super) fn err_code(code: &str, msg: impl Into<String>) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message: msg.into(),
        code: Some(code.to_string()),
    }
}

pub(super) fn schema_lock_key(db: &str, repo: &str, table: &str) -> String {
    format!("schema:{db}/{repo}/{table}")
}

pub(super) async fn schema_repo_permit(
    shamir: &ShamirDb,
    db_name: &str,
    repo_name: &str,
) -> Result<RepoWritePermit, BatchError> {
    let repo = shamir
        .get_db(db_name)
        .and_then(|db| db.get_repo(repo_name))
        .ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Repository '{db_name}.{repo_name}' not found"),
            code: Some("internal_error".to_string()),
        })?;
    repo.write_domain()
        .admit()
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("repository write admission failed: {e}"),
            code: Some("internal_error".to_string()),
        })
}

/// Begin the schema-activation write barrier for `table`: resolve the table
/// handle and acquire the barrier via
/// [`TableManager::begin_write_barrier`](crate::engine::table::TableManager::begin_write_barrier)
/// (`SCHEMA_ACTIVATION` bit) — raise the intent bit, drain in-flight
/// fast-path writers, THEN take `unique_write_lock`, in that order (F-70,
/// #897). Returns the table handle plus both RAII guards; the caller keeps
/// all three alive for the ENTIRE count→persist→activate sequence (drop
/// order: barrier clears first, then the lock releases — reverse declaration
/// order).
///
/// Call this immediately BEFORE the `stamp_keyset_safe` count-proof read so
/// the subsequent count→persist→activate sequence is a genuine snapshot no
/// concurrent writer can invalidate.
pub(super) async fn begin_schema_activation_barrier(
    shamir: &ShamirDb,
    db: &str,
    repo: &str,
    table: &str,
    permit: &RepoWritePermit,
) -> Result<
    (
        TableManager,
        crate::engine::table::WriteBarrierGuard,
        tokio::sync::OwnedMutexGuard<()>,
    ),
    BatchError,
> {
    let handle = shamir
        .get_table_admitted(db, repo, table, permit)
        .await
        .map_err(|e| err_code("internal_error", e.to_string()))?;
    let (barrier, uwl_guard) = handle
        .begin_write_barrier_admitted(
            crate::engine::index::write_barrier_flags::SCHEMA_ACTIVATION,
            permit,
        )
        .await;
    Ok((handle, barrier, uwl_guard))
}
