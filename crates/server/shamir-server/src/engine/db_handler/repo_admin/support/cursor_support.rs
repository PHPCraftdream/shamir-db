//! Shared cursor-path support helpers, extracted from `cursor_handlers.rs`
//! (P2#6 file ratchet — `cursor_handlers.rs` was over its master size
//! ratchet; these two module-level fns are the only self-contained pieces).
//!
//! Mounted from `db_handler/mod.rs` via `#[path = "repo_admin/cursor_support.rs"]`
//! (same pattern as `repo_audit` / `tx_handlers`), because `db_handler/`
//! itself is over its folder-entry limit while `repo_admin/` has room.

use shamir_db::engine::repo::RepoInstance;
use shamir_db::query::batch::BatchError;

use super::operator_state::table_unavailable_error;

/// Resolve `(db_name, query.from.repo)` down to a `RepoInstance`, mirroring
/// `tx_begin_as`'s `db.get_db(...)`/`db.get_repo(...)` idiom exactly
/// (`crates/engine/shamir-db/src/shamir_db/execute/db_tx.rs:70-81`).
///
/// CR-C1 (#776, B-5): both not-found cases set an EXPLICIT `code` rather
/// than leaving it `None` and relying on `error_code()`'s legacy
/// message-sniffing fallback (`alias.is_empty() && message.contains("not
/// found") -> "unknown_db"`). That heuristic could not tell the two cases
/// apart — a REPO-not-found error's message also contains "not found" with
/// an empty alias, so it was silently misclassified as `unknown_db` too
/// (misleading: the database exists, only the repo inside it doesn't).
/// `unknown_db` is kept for the db-not-found case (preserves today's actual
/// wire code, just makes it explicit instead of accidental); `unknown_repo`
/// is a new, distinct code for the repo-not-found case (grepped for
/// collisions against every other wire error code in the codebase before
/// introducing it — none found).
pub(super) fn resolve_repo(
    db: &shamir_db::ShamirDb,
    db_name: &str,
    repo_name: &str,
) -> Result<RepoInstance, BatchError> {
    let dbi = db.get_db(db_name).ok_or_else(|| BatchError::QueryError {
        alias: String::new(),
        message: format!("Database '{}' not found", db_name),
        code: Some("unknown_db".to_string()),
    })?;
    dbi.get_repo(repo_name)
        .ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Repository '{}' not found", repo_name),
            code: Some("unknown_repo".to_string()),
        })
}

/// Authorize `actor` for `Action::Read` on the target table, mirroring the
/// exact two-call shape `execute_as` uses for the normal batch path
/// (`crates/engine/shamir-db/src/shamir_db/execute/db_execute.rs::execute_as`,
/// ~lines 35-65): a `Database` check up front, then a `Table` check for the
/// specific target. `authorize_access`'s own ancestor-walk already covers the
/// `Store` link internally, so these two calls are sufficient — no more, no
/// less. `Actor::System`/`Actor::Admin` short-circuit to `Ok(())` inside
/// `authorize_access` itself (admin bypass, same as everywhere else).
///
/// **Refusal parity — no table-existence oracle.** The two calls answer
/// with DIFFERENT codes, deliberately:
/// - the `Database` denial keeps `access_denied`: it runs before any table
///   is resolved and therefore discloses nothing about one;
/// - the `Table` denial answers `unknown_table` — the SAME code every read
///   path answers for an absent table (`GetRecord`'s `table_unavailable_response`
///   and the configured-table refusals in
///   `crates/engine/shamir-engine/src/query/batch/exec/batch_validate.rs`).
///   The table ACL is open by default for a name with no catalogue record,
///   so a table-Read denial is only reachable for a table that EXISTS;
///   replying `access_denied` there would let the caller tell a closed table
///   from an absent one. The refusal carries the crate's single fixed
///   [`table_unavailable_error`] text (never the ACL's `e.to_string()`), so
///   the closed and absent cases are byte-identical on the wire.
///
/// `pub(super)`: also used by `handler.rs`'s `get_ddl_op_status` (#1064) —
/// a DDL-status poll is a read of that table's operational state, so it
/// gets the SAME table-Read gate a cursor read does. Authorizing BEFORE any
/// table/repo resolution (this function takes plain name strings, not
/// resolved handles) is what keeps an unauthorized caller's response
/// indistinguishable from a not-found response — see `get_ddl_op_status`'s
/// call site for why the ordering matters.
pub(super) async fn authorize_cursor_read(
    db: &shamir_db::ShamirDb,
    actor: &shamir_db::access::Actor,
    db_name: &str,
    repo_name: &str,
    table_name: &str,
) -> Result<(), BatchError> {
    db.authorize_access(
        actor,
        &shamir_db::access::ResourcePath::Database {
            db: db_name.to_string(),
        },
        shamir_db::access::Action::Read,
    )
    .await
    .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;

    db.authorize_access(
        actor,
        &shamir_db::access::ResourcePath::Table {
            db: db_name.to_string(),
            store: repo_name.to_string(),
            table: table_name.to_string(),
        },
        shamir_db::access::Action::Read,
    )
    .await
    .map_err(|_| table_unavailable_error())?;

    Ok(())
}
