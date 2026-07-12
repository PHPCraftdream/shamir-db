use std::sync::Arc;

use shamir_connect::common::crypto::random_array;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::session::Session;
use shamir_connect::server::user_record::UserRecord;
use shamir_db::query::batch::{BatchOp, BatchRequest};
use zeroize::Zeroizing;

use crate::tables_registry::TablesRegistry;
use crate::user_directory::FjallUserDirectory;

use super::handler::DbResponse;

/// Optional admin glue — supplied by the boot path so admin ops that
/// require server-side state (the SCRAM user directory + KDF cost
/// parameters + the wire-tables persistence registry) can run. Tests
/// that don't need any of these omit it via `ShamirDbHandler::new`.
#[derive(Clone)]
pub struct AdminGlue {
    /// Directory that stores SCRAM-authenticatable users.
    pub user_dir: Arc<FjallUserDirectory>,
    /// KDF defaults applied to newly created users so they can log in
    /// against the same listener policy.
    pub kdf: KdfParams,
    /// Tracks tables created/dropped over the wire so the boot path can
    /// re-register them on restart. `None` means "don't persist table
    /// changes" — fine for in-memory test setups, wrong for production.
    pub tables_registry: Option<Arc<TablesRegistry>>,
}

/// Create a SCRAM-authenticatable user. Server-side Argon2id is CPU-bound
/// and is delegated to `tokio::task::spawn_blocking` so the runtime worker
/// remains free during derivation.
pub(super) async fn create_scram_user(
    admin: Option<&AdminGlue>,
    session: &Session,
    name: String,
    password: String,
    roles: Vec<String>,
) -> DbResponse {
    if !session.permissions.is_superuser {
        return DbResponse::Error {
            code: "permission_denied".into(),
            message: "create_scram_user requires superuser".into(),
        };
    }
    let admin = match admin {
        Some(a) => a,
        None => {
            return DbResponse::Error {
                code: "not_supported".into(),
                message: "handler built without AdminGlue (no user_dir)".into(),
            }
        }
    };

    // Move password into a zeroizing buffer right away. `Zeroizing`
    // wipes on Drop, so we don't need an explicit `.zeroize()` call —
    // both the success and error paths drop `pw_buf` before returning.
    let pw_buf: Zeroizing<Vec<u8>> = Zeroizing::new(password.into_bytes());
    let salt: [u8; 16] = random_array();
    let kdf = admin.kdf;

    // Argon2id is CPU-heavy — delegate to spawn_blocking so the runtime
    // worker is free to make progress on other tasks during derivation.
    let derive_result = tokio::task::spawn_blocking(move || {
        DerivedKeys::derive(&pw_buf, &salt, &kdf).map(|d| (d, salt))
    })
    .await;

    let (derived, salt) = match derive_result {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            return DbResponse::Error {
                code: "query".into(),
                message: format!("argon2id: {e}"),
            };
        }
        Err(join_err) => {
            return DbResponse::Error {
                code: "query".into(),
                message: format!("argon2id task failed: {join_err}"),
            };
        }
    };

    let mut server_key_z: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
    server_key_z.copy_from_slice(&derived.server_key[..]);
    let record = UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: server_key_z,
        kdf_params: admin.kdf,
        tickets_invalid_before_ns: 0,
    };

    let user_id = match admin.user_dir.insert(name.clone(), record) {
        Ok(id) => id,
        Err(e) => {
            let msg = e.to_string();
            let code = if msg.contains("exists") {
                "user_exists"
            } else {
                "query"
            };
            return DbResponse::Error {
                code: code.into(),
                message: msg,
            };
        }
    };
    if !roles.is_empty() {
        // Best-effort role attach. now_ns=0 means "don't bump session
        // validity epoch" — no existing sessions for a brand-new user.
        let _ = admin.user_dir.update_roles(&name, roles, 0);
    }

    DbResponse::UserCreated {
        name,
        user_id: user_id.to_vec(),
    }
}

/// Walk the batch and verify the `hmac` tag on every destructive op.
///
/// Covers: `DropDb`/`DropRepo`/`DropTable`/`DropIndex`/`DropUser`/
/// `DropRole`, `Start/Commit/RollbackMigration`, `GrantRole`/`RevokeRole`
/// (the single most dangerous op class — privilege escalation),
/// `Chmod`/`Chown`/`Chgrp` (ownership/permission changes),
/// `CreateUser`/`CreateRole`, and `SetRetention`/`PurgeHistory`
/// (irreversible audit-trail loss). Group-mutating ops
/// (`CreateGroup`/`DropGroup`/`RenameGroup`/`Add|RemoveGroupMember`) are
/// NOT yet covered — see task #542's follow-up (audit ranks them lowest
/// severity of this cluster).
///
/// Returns `Err((alias, code, message))` on the first failure
/// where `code` is one of:
///   * `"hmac_required"` — the field is missing on a destructive op,
///   * `"hmac_mismatch"` — the field is present but the tag doesn't
///     match the recomputed value for this op + this session.
///
/// Non-destructive ops pass through untouched. Auth check has
/// already happened above; this gate runs strictly after that.
pub(super) fn check_destructive_hmacs(
    session: &Session,
    db_name: &str,
    batch: &BatchRequest,
) -> Result<(), (String, &'static str, String)> {
    use shamir_query_types::hmac as canon;

    // Lazy derive only when there's at least one destructive op.
    let mut key_opt: Option<[u8; 32]> = None;
    let key = |k: &mut Option<[u8; 32]>| -> [u8; 32] {
        if let Some(v) = *k {
            return v;
        }
        let derived = session.hmac_key();
        *k = Some(derived);
        derived
    };

    for (alias, entry) in &batch.queries {
        let (canonical, supplied): (Vec<u8>, Option<&String>) = match &entry.op {
            BatchOp::DropDb(op) => (canon::canonical_drop_db(&op.drop_db), op.hmac.as_ref()),
            BatchOp::DropRepo(op) => (
                canon::canonical_drop_repo(db_name, &op.drop_repo),
                op.hmac.as_ref(),
            ),
            BatchOp::DropTable(op) => (
                canon::canonical_drop_table(db_name, &op.repo, &op.drop_table),
                op.hmac.as_ref(),
            ),
            BatchOp::DropIndex(op) => (
                canon::canonical_drop_index(
                    db_name,
                    &op.repo,
                    &op.table,
                    &op.drop_index,
                    op.unique,
                ),
                op.hmac.as_ref(),
            ),
            BatchOp::DropUser(op) => (canon::canonical_drop_user(&op.drop_user), op.hmac.as_ref()),
            BatchOp::DropRole(op) => (canon::canonical_drop_role(&op.drop_role), op.hmac.as_ref()),
            BatchOp::StartMigration(op) => (
                canon::canonical_start_migration(
                    db_name,
                    &op.repo,
                    &op.start_migration,
                    &op.dst_repo,
                    &op.dst_engine,
                ),
                op.hmac.as_ref(),
            ),
            BatchOp::CommitMigration(op) => (
                canon::canonical_commit_migration(db_name, &op.commit_migration),
                op.hmac.as_ref(),
            ),
            BatchOp::RollbackMigration(op) => (
                canon::canonical_rollback_migration(db_name, &op.rollback_migration),
                op.hmac.as_ref(),
            ),
            BatchOp::GrantRole(op) => (
                canon::canonical_grant_role(&op.grant_role, &op.user),
                op.hmac.as_ref(),
            ),
            BatchOp::RevokeRole(op) => (
                canon::canonical_revoke_role(&op.revoke_role, &op.user),
                op.hmac.as_ref(),
            ),
            BatchOp::Chmod(op) => (canon::canonical_chmod(&op.chmod, op.mode), op.hmac.as_ref()),
            BatchOp::Chown(op) => (
                canon::canonical_chown(&op.chown, op.owner),
                op.hmac.as_ref(),
            ),
            BatchOp::Chgrp(op) => (
                canon::canonical_chgrp(&op.chgrp, op.group),
                op.hmac.as_ref(),
            ),
            BatchOp::CreateUser(op) => (
                canon::canonical_create_user(&op.create_user),
                op.hmac.as_ref(),
            ),
            BatchOp::CreateRole(op) => (
                canon::canonical_create_role(&op.create_role),
                op.hmac.as_ref(),
            ),
            BatchOp::SetRetention(op) => (
                canon::canonical_set_retention(db_name, &op.repo, &op.set_retention, &op.retention),
                op.hmac.as_ref(),
            ),
            BatchOp::PurgeHistory(op) => (
                canon::canonical_purge_history(db_name, &op.repo, &op.purge_history, &op.scope),
                op.hmac.as_ref(),
            ),
            _ => continue, // non-destructive — pass.
        };

        let Some(tag) = supplied else {
            return Err((
                alias.clone(),
                "hmac_required",
                "destructive op missing `hmac` field".to_string(),
            ));
        };
        if !canon::verify_tag_hex(&key(&mut key_opt), &canonical, tag) {
            return Err((
                alias.clone(),
                "hmac_mismatch",
                "destructive op `hmac` does not match canonical input".to_string(),
            ));
        }
    }
    Ok(())
}
