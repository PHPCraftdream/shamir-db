//! Admin command execution layer (spec §12).
//!
//! All commands require `is_superuser == true` in the calling session's
//! [`SessionPermissions`]. Each command is atomic with respect to the
//! per-user mutex pattern (caller wires that). Audit events are emitted via
//! a pluggable callback so the application's audit log subsystem can pick
//! them up.

use crate::common::crypto::StoredKey;
use crate::common::error::{Error, Result};
use crate::common::kdf_params::KdfParams;
use crate::common::types::limits;
use crate::server::session::{Session, SessionStore};
use crate::server::user_record::UserRecord;
use std::sync::Arc;
use zeroize::Zeroizing;

/// Minimal trait for the user-storage backend used by admin commands.
///
/// Real impl wires this to the project's `__system__/users` collection.
pub trait UserDirectory: Send + Sync {
    /// Look up a user record by username.
    fn lookup_by_name(&self, username: &str) -> Option<UserRecord>;

    /// Insert a new user record. Returns the assigned `user_id`.
    fn insert(&self, username: String, record: UserRecord) -> Result<[u8; 16]>;

    /// Update `roles` (and `tickets_invalid_before_ns`) for a user.
    /// Returns true if any field changed.
    fn update_roles(&self, username: &str, roles: Vec<String>, now_ns: u64) -> Result<bool>;

    /// Bump `tickets_invalid_before_ns` for a user without changing roles.
    fn bump_tickets_invalid(&self, username: &str, now_ns: u64) -> Result<bool>;

    /// Look up `user_id` by username.
    fn user_id(&self, username: &str) -> Option<[u8; 16]>;
}

/// Audit event hook — emitted for every admin command (spec IMPL §3.2).
pub trait AuditSink: Send + Sync {
    /// Record a single event. `details` is application-defined.
    fn emit(&self, event: &str, actor: &str, details: &[(&str, &str)]);
}

/// Helper: enforce admin authorization on the calling session.
fn require_superuser(session: &Session) -> Result<()> {
    if session.permissions.read().is_superuser {
        Ok(())
    } else {
        Err(Error::AuthFailed)
    }
}

// -----------------------------------------------------------------
// createUser (spec §12.1)
// -----------------------------------------------------------------

/// Inputs for [`create_user`].
#[derive(Debug, Clone)]
pub struct CreateUserInput {
    /// New user's username (post-NFC, UsernameCaseMapped — caller validates).
    pub username: String,
    /// Per-user salt.
    pub salt: [u8; limits::SALT_BYTES],
    /// Stored key (= SHA256(client_key)).
    pub stored_key: [u8; 32],
    /// Server key.
    pub server_key: [u8; 32],
    /// KDF parameters used.
    pub kdf_params: KdfParams,
    /// Roles to assign.
    pub roles: Vec<String>,
}

/// Create a new user. Returns the assigned `user_id`.
pub fn create_user<D: UserDirectory, A: AuditSink>(
    actor: &Session,
    input: CreateUserInput,
    current_kdf_params: &KdfParams,
    directory: &D,
    audit: &A,
) -> Result<[u8; 16]> {
    require_superuser(actor)?;

    if directory.lookup_by_name(&input.username).is_some() {
        return Err(Error::InvalidInput("user already exists"));
    }
    if &input.kdf_params != current_kdf_params {
        return Err(Error::InvalidInput("kdf_params != server defaults"));
    }
    input
        .kdf_params
        .validate_server_floor()
        .map_err(|_| Error::InvalidInput("kdf_params below floor"))?;

    let mut server_key_buf = Zeroizing::new([0u8; 32]);
    server_key_buf.copy_from_slice(&input.server_key);

    let record = UserRecord {
        salt: input.salt,
        stored_key: StoredKey(input.stored_key),
        server_key: server_key_buf,
        kdf_params: input.kdf_params,
        tickets_invalid_before_ns: 0,
    };

    let new_uid = directory.insert(input.username.clone(), record)?;
    audit.emit(
        "user_created",
        &actor.username,
        &[("user", &input.username)],
    );
    Ok(new_uid)
}

// -----------------------------------------------------------------
// kickSession (spec §12.4)
// -----------------------------------------------------------------

/// Outcome of [`kick_session`].
#[derive(Debug, Clone)]
pub struct KickSessionResult {
    /// Number of sessions evicted.
    pub killed_count: u32,
}

/// Kill all sessions of a user AND set `tickets_invalid_before_ns = now_ns`.
///
/// Atomic per spec §12.4: persist barrier between bumping the timestamp and
/// snapshotting+killing sessions.
pub fn kick_session<D: UserDirectory, A: AuditSink>(
    actor: &Session,
    target_username: &str,
    now_ns: u64,
    directory: &D,
    sessions: &SessionStore,
    audit: &A,
) -> Result<KickSessionResult> {
    require_superuser(actor)?;

    let target_uid = directory
        .user_id(target_username)
        .ok_or(Error::InvalidInput("user not found"))?;

    // Step 1: bump tickets_invalid_before_ns first (persist barrier).
    let _ = directory.bump_tickets_invalid(target_username, now_ns)?;

    // Step 2: snapshot + kill.
    let victims = sessions.snapshot_by_user(&target_uid);
    let mut killed = 0u32;
    for sid in victims {
        if sessions.remove(&sid).is_some() {
            killed += 1;
        }
    }

    audit.emit(
        "kick_session",
        &actor.username,
        &[
            ("target", target_username),
            ("killed", &killed.to_string()),
        ],
    );

    Ok(KickSessionResult { killed_count: killed })
}

// -----------------------------------------------------------------
// updateUser (spec §12.6)
// -----------------------------------------------------------------

/// Outcome of [`update_user`].
#[derive(Debug, Clone)]
pub struct UpdateUserResult {
    /// Whether anything actually changed (no-op semantic per spec §12.6).
    pub changes_applied: bool,
}

/// Update a user's roles atomically.
///
/// **No-op semantic:** if `roles == None` AND the user record doesn't change,
/// returns `{changes_applied: false}` WITHOUT bumping `tickets_invalid_before_ns`.
/// Defends against silent-DoS via repeated noop updateUser calls.
pub fn update_user<D: UserDirectory, A: AuditSink>(
    actor: &Session,
    target_username: &str,
    new_roles: Option<Vec<String>>,
    now_ns: u64,
    directory: &D,
    sessions: &SessionStore,
    audit: &A,
) -> Result<UpdateUserResult> {
    require_superuser(actor)?;

    let target_uid = directory
        .user_id(target_username)
        .ok_or(Error::InvalidInput("user not found"))?;

    let new_roles = match new_roles {
        Some(r) => r,
        None => {
            // No-op — nothing changed.
            audit.emit(
                "update_user_noop",
                &actor.username,
                &[("user", target_username)],
            );
            return Ok(UpdateUserResult {
                changes_applied: false,
            });
        }
    };

    let changed = directory.update_roles(target_username, new_roles, now_ns)?;
    if !changed {
        audit.emit(
            "update_user_noop",
            &actor.username,
            &[("user", target_username)],
        );
        return Ok(UpdateUserResult {
            changes_applied: false,
        });
    }

    // Snapshot+kill sessions of the user (best-effort eager eviction).
    let victims = sessions.snapshot_by_user(&target_uid);
    for sid in victims {
        sessions.remove(&sid);
    }

    audit.emit(
        "roles_changed",
        &actor.username,
        &[("user", target_username)],
    );

    Ok(UpdateUserResult {
        changes_applied: true,
    })
}

// -----------------------------------------------------------------
// unlockUser (spec §12.3)
// -----------------------------------------------------------------

/// Reset `auth_failures` AND `lockout_state` for a user across all subnets.
///
/// `clear_state_for_user` is a hook because the lockout state lives in the
/// application's auth-failure store (separate from this crate).
pub fn unlock_user<A: AuditSink, F: FnOnce(&str)>(
    actor: &Session,
    target_username: &str,
    clear_state_for_user: F,
    audit: &A,
) -> Result<()> {
    require_superuser(actor)?;
    clear_state_for_user(target_username);
    audit.emit(
        "lockout_released",
        &actor.username,
        &[("user", target_username)],
    );
    Ok(())
}

// -----------------------------------------------------------------
// In-memory implementations for tests + small deployments
// -----------------------------------------------------------------

/// Reference in-memory implementation of [`UserDirectory`].
#[derive(Debug, Default)]
pub struct InMemoryUserDirectory {
    by_name: dashmap::DashMap<String, ([u8; 16], UserRecord)>,
    next_id: parking_lot::Mutex<u128>,
}

impl InMemoryUserDirectory {
    /// Empty directory.
    pub fn new() -> Self {
        Self {
            by_name: dashmap::DashMap::new(),
            next_id: parking_lot::Mutex::new(1),
        }
    }

    /// Pre-insert a user with a deterministic id (for tests).
    pub fn preinsert(&self, username: String, user_id: [u8; 16], record: UserRecord) {
        self.by_name.insert(username, (user_id, record));
    }
}

impl UserDirectory for InMemoryUserDirectory {
    fn lookup_by_name(&self, username: &str) -> Option<UserRecord> {
        self.by_name.get(username).map(|r| r.value().1.clone())
    }

    fn insert(&self, username: String, record: UserRecord) -> Result<[u8; 16]> {
        if self.by_name.contains_key(&username) {
            return Err(Error::InvalidInput("user already exists"));
        }
        let mut nid = self.next_id.lock();
        let id_u128 = *nid;
        *nid += 1;
        let mut user_id = [0u8; 16];
        user_id.copy_from_slice(&id_u128.to_be_bytes());
        self.by_name.insert(username, (user_id, record));
        Ok(user_id)
    }

    fn update_roles(&self, _username: &str, _roles: Vec<String>, _now_ns: u64) -> Result<bool> {
        // UserRecord in this crate doesn't carry roles; the application stores
        // them. For test purposes we just bump tickets_invalid_before_ns.
        self.bump_tickets_invalid(_username, _now_ns)
    }

    fn bump_tickets_invalid(&self, username: &str, now_ns: u64) -> Result<bool> {
        if let Some(mut entry) = self.by_name.get_mut(username) {
            let (_uid, record) = entry.value_mut();
            if record.tickets_invalid_before_ns >= now_ns {
                return Ok(false);
            }
            record.tickets_invalid_before_ns = now_ns;
            Ok(true)
        } else {
            Err(Error::InvalidInput("user not found"))
        }
    }

    fn user_id(&self, username: &str) -> Option<[u8; 16]> {
        self.by_name.get(username).map(|r| r.value().0)
    }
}

/// Reference in-memory [`AuditSink`] — logs events into a `Vec`.
#[derive(Debug, Default)]
pub struct InMemoryAuditSink {
    events: parking_lot::Mutex<Vec<AuditEvent>>,
}

/// Recorded audit event.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    /// Event name (e.g. "user_created").
    pub event: String,
    /// Actor (username of the admin executing the command).
    pub actor: String,
    /// Key-value details, owned strings.
    pub details: Vec<(String, String)>,
}

impl InMemoryAuditSink {
    /// Empty sink.
    pub fn new() -> Self {
        Self {
            events: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Snapshot all collected events.
    pub fn snapshot(&self) -> Vec<AuditEvent> {
        self.events.lock().clone()
    }
}

impl AuditSink for InMemoryAuditSink {
    fn emit(&self, event: &str, actor: &str, details: &[(&str, &str)]) {
        self.events.lock().push(AuditEvent {
            event: event.to_string(),
            actor: actor.to_string(),
            details: details
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });
    }
}

/// Convenience: wrap any value in `Arc` for shared admin-API ownership.
pub fn shared<T>(t: T) -> Arc<T> {
    Arc::new(t)
}
