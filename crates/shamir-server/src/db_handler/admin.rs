use std::sync::Arc;

use shamir_connect::common::crypto::random_array;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::limits;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::changepw::{
    finalize_change_password, start_change_password_challenge,
    verify_change_password_request_with_sid, ChangePwRequest,
};
use shamir_connect::server::session::{Session, SessionStore};
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

/// `ChangePasswordChallenge` (spec §12.5 step 1) — issue a fresh
/// server-side challenge bound to the caller's own session.
///
/// Authorization: none beyond "you hold a valid session" — the caller can
/// only ever change their own password (proven by the SCRAM proof-of-old-
/// password in [`change_password_verify`]), so there is nothing extra to
/// gate here (per the task brief: adding a role/permission check would be
/// redundant with, or could conflict with, the SCRAM verification itself).
pub(super) async fn change_password_challenge(
    admin: Option<&AdminGlue>,
    session: &Session,
    client_nonce_cp: Vec<u8>,
) -> DbResponse {
    let admin = match admin {
        Some(a) => a,
        None => {
            return DbResponse::Error {
                code: "not_supported".into(),
                message: "handler built without AdminGlue (no user_dir)".into(),
            }
        }
    };
    let Some(nonce) = as_array_32(&client_nonce_cp) else {
        return DbResponse::Error {
            code: "validation".into(),
            message: "client_nonce_cp must be 32 bytes".into(),
        };
    };

    let Some(record) = admin.user_dir.lookup_by_name(&session.username) else {
        // The session's own user vanished (e.g. concurrently deleted).
        // Treat as auth failure rather than leaking existence info.
        return DbResponse::Error {
            code: "auth_failed".into(),
            message: "user not found".into(),
        };
    };

    let now_ns = UnixNanos::now().as_u64();
    let view =
        start_change_password_challenge(session, record.salt, record.kdf_params, nonce, now_ns);

    DbResponse::ChangePasswordChallenge {
        server_nonce_cp: view.server_nonce_cp.to_vec(),
        salt: view.salt.to_vec(),
        kdf_memory_kb: view.kdf_params.memory_kb,
        kdf_time: view.kdf_params.time,
        kdf_parallelism: view.kdf_params.parallelism,
        kdf_argon2_version: view.kdf_params.argon2_version,
    }
}

/// `ChangePasswordVerify` (spec §12.5 step 2) — verify the old-password
/// SCRAM proof, and on success persist the new credentials, bump
/// `tickets_invalid_before_ns`, and kill every other live session for this
/// user (spec §12.5.3).
///
/// `session_store` is threaded in from [`super::handler::ShamirDbHandler`]'s
/// own `Arc<SessionStore>` field (see `ShamirDbHandler::with_session_store`)
/// — it is only needed here, for the session-kill half of this flow.
#[allow(clippy::too_many_arguments)]
pub(super) async fn change_password_verify(
    admin: Option<&AdminGlue>,
    session_store: Option<&Arc<SessionStore>>,
    session: &Session,
    client_proof_old: Vec<u8>,
    new_salt: Vec<u8>,
    new_stored_key: Vec<u8>,
    new_server_key: Vec<u8>,
) -> DbResponse {
    let admin = match admin {
        Some(a) => a,
        None => {
            return DbResponse::Error {
                code: "not_supported".into(),
                message: "handler built without AdminGlue (no user_dir)".into(),
            }
        }
    };
    let (Some(proof_old), Some(salt_new), Some(stored_key_new), Some(server_key_new)) = (
        as_array_32(&client_proof_old),
        as_array_16(&new_salt),
        as_array_32(&new_stored_key),
        as_array_32(&new_server_key),
    ) else {
        return DbResponse::Error {
            code: "validation".into(),
            message: "changePasswordVerify: wrong field length \
                      (client_proof_old/new_stored_key/new_server_key=32 bytes, new_salt=16 bytes)"
                .into(),
        };
    };

    let Some(record) = admin.user_dir.lookup_by_name(&session.username) else {
        return DbResponse::Error {
            code: "auth_failed".into(),
            message: "user not found".into(),
        };
    };

    let request = ChangePwRequest {
        client_proof_old: proof_old,
        new_salt: salt_new,
        new_stored_key: stored_key_new,
        new_server_key: server_key_new,
    };

    let now_ns = UnixNanos::now().as_u64();
    let apply = match verify_change_password_request_with_sid(
        session,
        &session.session_id,
        record.salt,
        &record.stored_key,
        record.kdf_params,
        &request,
        // `current_kdf_params` is what gets persisted verbatim as the
        // user's NEW `kdf_params` (see `ChangePwApply::kdf_params` doc
        // comment). This MUST be the user's own current `record.kdf_params`
        // — the same value echoed in the `ChangePasswordChallenge` response
        // — NOT the server's global `admin.kdf` default for brand-new
        // users. The two coincide today (single global KDF policy, no
        // per-user override), but diverge the moment KDF params are
        // rotated or tuned per-user: passing `admin.kdf` here would
        // persist the WRONG kdf_params alongside stored_key/server_key
        // that the client actually derived under `record.kdf_params`,
        // corrupting the user's next login.
        record.kdf_params,
        now_ns,
    ) {
        Ok(apply) => apply,
        Err(_) => {
            return DbResponse::Error {
                code: "auth_failed".into(),
                message: "changePasswordVerify: proof_old verification failed".into(),
            };
        }
    };

    // Persist the new credentials AND bump `tickets_invalid_before_ns` in
    // the SAME read-modify-write transaction (see `FjallUserDirectory::
    // update_credentials` doc comment for why this must be one atomic
    // write rather than a credential write followed by a separate
    // `bump_tickets_invalid` call).
    if let Err(e) = admin.user_dir.update_credentials(
        &session.username,
        apply.salt,
        apply.stored_key,
        *apply.server_key,
        apply.kdf_params,
        now_ns,
    ) {
        return DbResponse::Error {
            code: "query".into(),
            message: format!("changePasswordVerify: persist failed: {e}"),
        };
    }

    // Kill every other live session for this user (spec §12.5.3: "Все
    // сессии юзера убиваются (включая текущую)"). Only reachable when the
    // handler was constructed with a `SessionStore` (see Gap 2 in the task
    // brief); if absent, the credential update above still lands — the
    // ticket-revocation half is a documented partial-fix fallback.
    if let Some(store) = session_store {
        let _ = finalize_change_password(store, &session.user_id, now_ns);
    }

    DbResponse::ChangePasswordOk
}

/// Explicit allowlist of admin/DDL ops exempted from the coarse wire-admin
/// gate (task #553, per `docs/design/root-user-group-dac-posture-550-decision.md`
/// §2). Exactly the 4 ops the design decision named — `List`, `AccessTree`,
/// `DescribeTable`, `GetTableSchema` — never derived from `is_write()` or
/// any other classifier.
///
/// Each of these 4 ops still runs its OWN independent per-table/per-path
/// `authorize_access` check further down the stack (`admin_list.rs`,
/// `admin_access.rs`'s `handle_access_tree`, `admin_describe.rs`,
/// `admin_schema.rs`) — this predicate only stops the coarse gate from
/// blocking them outright; it grants nothing by itself.
///
/// `BatchOp::Batch` (nested sub-batch) MUST NEVER be added here: its
/// `required_access` is `None` (`batch_op.rs:543`), so the per-op
/// authorization loop never recurses into a sub-batch's nested queries.
/// Exempting `Batch` would let `Batch{ Read(forbidden_table) }` execute
/// with zero per-table authorization — reopening the bug class task #510
/// closed for `Subscribe`. The other 8 ops that `is_write() == false`
/// covers (`GetBufferConfig`, `MigrationStatus`, `InternerDump`,
/// `ChangesSince`, `ListValidators`, `ListPublications`,
/// `ListSubscriptions`, `ReplicationStatus`) are deliberately excluded too
/// — extending the exemption to any of them is a separate, deliberate
/// decision to be made individually, not swept in by a blanket classifier.
pub(super) fn is_coarse_admin_gate_exempt(op: &BatchOp) -> bool {
    matches!(
        op,
        BatchOp::List(_)
            | BatchOp::AccessTree(_)
            | BatchOp::DescribeTable(_)
            | BatchOp::GetTableSchema(_)
    )
}

fn as_array_32(bytes: &[u8]) -> Option<[u8; 32]> {
    <[u8; 32]>::try_from(bytes).ok()
}

fn as_array_16(bytes: &[u8]) -> Option<[u8; limits::SALT_BYTES]> {
    <[u8; limits::SALT_BYTES]>::try_from(bytes).ok()
}

/// Walk the batch and verify the `hmac` tag on every destructive op.
///
/// Covers: `DropDb`/`DropRepo`/`DropTable`/`DropIndex`/`DropUser`/
/// `DropRole`, `Start/Commit/RollbackMigration`, `GrantRole`/`RevokeRole`
/// (the single most dangerous op class — privilege escalation),
/// `Chmod`/`Chown`/`Chgrp` (ownership/permission changes),
/// `CreateUser`/`CreateRole`, `SetRetention`/`PurgeHistory`
/// (irreversible audit-trail loss), and the group-mutating ops
/// (`CreateGroup`/`DropGroup`/`RenameGroup`/`Add|RemoveGroupMember`).
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
            BatchOp::CreateGroup(op) => (
                canon::canonical_create_group(&op.create_group),
                op.hmac.as_ref(),
            ),
            BatchOp::DropGroup(op) => (
                canon::canonical_drop_group(&op.drop_group),
                op.hmac.as_ref(),
            ),
            BatchOp::RenameGroup(op) => (
                canon::canonical_rename_group(&op.rename_group, &op.to),
                op.hmac.as_ref(),
            ),
            BatchOp::AddGroupMember(op) => (
                canon::canonical_add_group_member(&op.add_group_member, op.user),
                op.hmac.as_ref(),
            ),
            BatchOp::RemoveGroupMember(op) => (
                canon::canonical_remove_group_member(&op.remove_group_member, op.user),
                op.hmac.as_ref(),
            ),
            BatchOp::CreateFunction(op) => {
                // CONDITIONAL HMAC (unique among the arms): the tag is
                // required IFF `security == "definer"` or `secret_grants`
                // is non-empty. A plain create_function (the common case)
                // needs no tag at all — `continue` skips it exactly like
                // the `_ => continue` fallthrough for non-destructive ops.
                let needs_hmac =
                    op.security.as_deref() == Some("definer") || !op.secret_grants.is_empty();
                if !needs_hmac {
                    continue;
                }
                (
                    canon::canonical_create_function(
                        &op.create_function,
                        op.security.as_deref().unwrap_or("invoker"),
                        &op.secret_grants,
                    ),
                    op.hmac.as_ref(),
                )
            }
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
