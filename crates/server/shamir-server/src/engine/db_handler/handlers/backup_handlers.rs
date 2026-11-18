//! Online-backup (R2a step 3a, design D6) follower-gate helpers for the
//! db_handler.
//!
//! The gate logic (dispatch and placement) stays in `handler.rs`; only the
//! refusal predicate and error body live here, so `handler.rs` remains
//! under the repo layout gate's 1000-line cap. No behaviour change.

use shamir_db::query::batch::BatchOp;

use crate::db_handler::handler::DbResponse;

/// Follower read-only refusal for any client write on a read-only replica.
///
/// Node-local online-backup writes (`StartBackup` / `CancelBackup`) get the
/// stable `online_backup_follower_read_only` code (R2a step 3a, design D6);
/// every other write keeps the coarse `read_only_replica` code. `BackupStatus`
/// is read-only and never reaches this gate. No behaviour change.
pub(crate) fn follower_write_refusal(op: &BatchOp, alias: &str) -> DbResponse {
    if matches!(op, BatchOp::StartBackup(_) | BatchOp::CancelBackup(_)) {
        return DbResponse::Error {
            code: "online_backup_follower_read_only".into(),
            message: format!(
                "query '{}': online-backup start/cancel are refused on a \
                 follower/read replica (node-local write)",
                alias
            ),
        };
    }
    DbResponse::Error {
        code: "read_only_replica".into(),
        message: format!(
            "query '{}' is a write; this node is a read-only replica",
            alias
        ),
    }
}
