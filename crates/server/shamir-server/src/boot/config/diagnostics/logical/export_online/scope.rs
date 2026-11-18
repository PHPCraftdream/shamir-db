//! Scope authorization and warm-open for the online logical-export job.
//!
//! Design: `docs/dev-artifacts/design/logical-dump.md` §4.1 ("superuser
//! authorization plus read/manage ACL checks for all included objects;
//! any denial aborts"). Session-level superuser and SCRAM completion are
//! verified in the dispatch layer before this module runs; here the
//! *current directory state* is re-checked (`state_by_user_id`) and every
//! included Db/Store/Table is authorized as
//! `Actor::User(principal64(user_id))` for both `Read` and `Manage`.
//!
//! Root-wide authority comes from the superuser requirement itself — the
//! gate's `Actor::Admin` bypass is never relied on and no owner-only
//! `Root` `Manage` check is imposed (that would make exports impossible
//! for a normal boot admin whose session reaches this point as a plain
//! principal). `Root` and `FunctionNamespace` are authorized for `Read`
//! as an ordinary actor so the dump's carried settings (`root_meta`,
//! `fn_namespace_meta`) and function-namespace ACL stay consistent with
//! the live gate; both tolerate the code's default metadata.
//!
//! Both functions are side-effect-free apart from warm table opens
//! (`warm_scope`): they allocate nothing proportional to row counts and
//! never materialize records. The runner invokes `warm_scope` before the
//! maintenance freeze under mutation admission and re-authorizes the
//! scope after the freeze, closing the authorize→freeze race.

use shamir_db::access::{principal64, Action, Actor, ResourcePath};
use shamir_db::engine::db_instance::db_instance::DbInstance;
use shamir_db::ShamirDb;
use shamir_query_types::admin::StartExportOp;
use shamir_server_security::user_directory::FjallUserDirectory;

/// Typed scope outcome. Static variants only: no peer text, no filesystem
/// paths, no catalogue error strings are carried on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScopeError {
    /// The current directory state does not carry the superuser flag (or
    /// the user is unknown): the job refuses before any gate is taken.
    Denied,
    /// The requested scope is malformed or unresolved: empty or oversized
    /// names, `table` without `repo`, the internal `__system__` database,
    /// or a database/repository/table that does not resolve.
    InvalidScope,
    /// The scope or options carry content this phase cannot represent
    /// (`include_artifacts`) or an inventory beyond the format's bounds.
    Unsupported,
    /// A catalogue or directory read failed while resolving the scope,
    /// or the scope is temporarily unexportable (quarantined repository,
    /// rename in doubt).
    Unavailable,
}

/// Largest single logical name accepted (design §3.1 bounded inventory;
/// per-name bound is deliberate defense, the format cap is 65,536).
const MAX_NAME_LEN: usize = 1024;

/// Largest inventory accepted before listing names: repositories per
/// database and tables per repository. Matches the format's 65,536
/// entries-per-container bound (design §3.1) so listing never allocates
/// beyond a legal section inventory.
const MAX_INVENTORY: usize = 65_536;

/// Validate one optional scope name: present, nonempty, bounded.
fn check_name(name: Option<&str>) -> Result<&str, ScopeError> {
    match name {
        None => Ok(""),
        Some(name) if name.is_empty() => Err(ScopeError::InvalidScope),
        Some(name) if name.len() > MAX_NAME_LEN => Err(ScopeError::InvalidScope),
        Some(name) => Ok(name),
    }
}

/// Authorize the requested export scope against the *current* directory
/// and ACL state and return the live [`DbInstance`] handle for the
/// requested database.
///
/// Ordering matters: structural validation and option refusals come
/// first, the superuser re-check second, resolution third, ACL checks
/// last — a denial or invalid scope never touches the maintenance gate
/// (the runner takes it only after this returns `Ok`).
pub(crate) async fn authorize_scope(
    home: &ShamirDb,
    directory: &FjallUserDirectory,
    user_id: [u8; 16],
    options: &StartExportOp,
) -> Result<DbInstance, ScopeError> {
    // ── structural validation before any catalogue or directory read ──
    let repo = check_name(options.repo.as_deref())?;
    let table = check_name(options.table.as_deref())?;
    if !table.is_empty() && repo.is_empty() {
        // Same rule as the offline command: `--table` requires `--repo`.
        return Err(ScopeError::InvalidScope);
    }
    if options.database.is_empty() || options.database.len() > MAX_NAME_LEN {
        return Err(ScopeError::InvalidScope);
    }
    if options.database == "__system__" {
        // The internal system database is never exportable (offline
        // precedent: `logical_dump/driver.rs` refuses it outright).
        return Err(ScopeError::InvalidScope);
    }
    if options.include_artifacts {
        // Guest-artifact support is out of scope in this phase; the wire
        // flag is accepted and refused closed, per the DTO contract.
        return Err(ScopeError::Unsupported);
    }

    // ── current directory state: superuser re-check ──
    // A bounded single-user read; the HTTP launcher calls
    // `state_by_user_id` inline on the same shared handle.
    let state = directory
        .state_by_user_id(&user_id)
        .ok_or(ScopeError::Denied)?;
    if !state.superuser {
        return Err(ScopeError::Denied);
    }

    // ── resolve the database handle ──
    let db = home
        .get_db(&options.database)
        .ok_or(ScopeError::InvalidScope)?;
    // Inventory bounds before any name materialization: the sum check
    // keeps `list_repos`/`list_table_names` allocations inside one legal
    // section inventory.
    if db.repo_count() > MAX_INVENTORY || db.table_count() > MAX_INVENTORY {
        return Err(ScopeError::Unsupported);
    }

    // The actor for all included-scope checks. `Actor::Admin` would bypass
    // the gate silently, but the session reaches this point as a plain
    // principal: the superuser requirement is enforced above, from the
    // directory, and Root-wide authority comes from it — not from an
    // owner-only Root `Manage` check (which would render exports
    // impossible for a normal boot admin).
    let actor = Actor::User(principal64(user_id));

    // Carried settings live under the root (`root_meta`,
    // `fn_namespace_meta`) and functions under the namespace: authorize
    // both for `Read` as an ordinary actor, tolerating the code's default
    // metadata for both singletons.
    home.authorize_access(&actor, &ResourcePath::Root, Action::Read)
        .await
        .map_err(|_| ScopeError::Denied)?;
    home.authorize_access(&actor, &ResourcePath::FunctionNamespace, Action::Read)
        .await
        .map_err(|_| ScopeError::Denied)?;

    // The database itself is an included object: `Read` for the export,
    // `Manage` for the definitions/security state carried with it. Both
    // checks precede repository processing.
    let db_path = ResourcePath::database(&options.database);
    home.authorize_access(&actor, &db_path, Action::Read)
        .await
        .map_err(|_| ScopeError::Denied)?;
    home.authorize_access(&actor, &db_path, Action::Manage)
        .await
        .map_err(|_| ScopeError::Denied)?;

    // ── included repositories ──
    let repo_names: Vec<String> = if repo.is_empty() {
        db.list_repos()
    } else {
        if !db.has_repo(repo) {
            return Err(ScopeError::InvalidScope);
        }
        vec![repo.to_owned()]
    };
    for repo_name in &repo_names {
        if repo_name.is_empty() || repo_name.len() > MAX_NAME_LEN {
            return Err(ScopeError::InvalidScope);
        }
        // Quarantine (resync_required) and a rename left in doubt are
        // temporary, unexportable states of an otherwise valid scope.
        if home
            .read_quarantine_reason(&options.database, repo_name)
            .is_some()
        {
            return Err(ScopeError::Unavailable);
        }
        let repo_handle = db.get_repo(repo_name).ok_or(ScopeError::InvalidScope)?;
        if repo_handle.has_rename_in_doubt() {
            return Err(ScopeError::Unavailable);
        }
        if repo_handle.table_count() > MAX_INVENTORY {
            return Err(ScopeError::Unsupported);
        }
        let db_path = ResourcePath::store(&options.database, repo_name);
        home.authorize_access(&actor, &db_path, Action::Read)
            .await
            .map_err(|_| ScopeError::Denied)?;
        home.authorize_access(&actor, &db_path, Action::Manage)
            .await
            .map_err(|_| ScopeError::Denied)?;

        // ── included tables ──
        let table_names: Vec<String> = if table.is_empty() {
            repo_handle.list_table_names()
        } else {
            if !repo_handle.has_table(table) {
                return Err(ScopeError::InvalidScope);
            }
            vec![table.to_owned()]
        };
        for table_name in &table_names {
            if table_name.is_empty() || table_name.len() > MAX_NAME_LEN {
                return Err(ScopeError::InvalidScope);
            }
            let table_path = ResourcePath::table(&options.database, repo_name, table_name);
            home.authorize_access(&actor, &table_path, Action::Read)
                .await
                .map_err(|_| ScopeError::Denied)?;
            home.authorize_access(&actor, &table_path, Action::Manage)
                .await
                .map_err(|_| ScopeError::Denied)?;
        }
    }

    Ok(db)
}

/// Warm-open exactly the tables the export will read, before the
/// maintenance freeze is taken (cold table opens run store opens, index
/// loads and possible repair — all of which must not happen inside the
/// freeze). Called under mutation admission by the runner; the scope was
/// already authorized by [`authorize_scope`] and is re-authorized after
/// the freeze to close the race. No side effects beyond the warm opens.
pub(crate) async fn warm_scope(db: &DbInstance, options: &StartExportOp) -> Result<(), ScopeError> {
    // Structural validation mirrors `authorize_scope` so the runner can
    // warm before taking the gate without re-running ACL checks.
    let repo = check_name(options.repo.as_deref())?;
    let table = check_name(options.table.as_deref())?;
    if !table.is_empty() && repo.is_empty() {
        return Err(ScopeError::InvalidScope);
    }

    let repo_names: Vec<String> = if repo.is_empty() {
        if db.repo_count() > MAX_INVENTORY || db.table_count() > MAX_INVENTORY {
            return Err(ScopeError::Unsupported);
        }
        db.list_repos()
    } else {
        if !db.has_repo(repo) {
            return Err(ScopeError::InvalidScope);
        }
        vec![repo.to_owned()]
    };
    for repo_name in &repo_names {
        let repo_handle = db.get_repo(repo_name).ok_or(ScopeError::InvalidScope)?;
        if repo_handle.table_count() > MAX_INVENTORY {
            return Err(ScopeError::Unsupported);
        }
        let table_names: Vec<String> = if table.is_empty() {
            repo_handle.list_table_names()
        } else {
            if !repo_handle.has_table(table) {
                return Err(ScopeError::InvalidScope);
            }
            vec![table.to_owned()]
        };
        for table_name in &table_names {
            db.get_table(repo_name, table_name)
                .await
                .map_err(|_| ScopeError::Unavailable)?;
        }
    }
    Ok(())
}
