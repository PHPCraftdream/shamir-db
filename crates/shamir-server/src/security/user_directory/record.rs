//! Persisted record format for the fjall-backed user directory: the
//! msgpack blob stored in `users_v1`, the reverse-index state snapshot,
//! and the pure boot-time `principal64` projection step.

use serde::{Deserialize, Serialize};
use shamir_collections::new_fx_map_wc;
use shamir_types::access::principal64;
use std::result::Result as StdResult;
use zeroize::Zeroizing;

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::user_record::UserRecord;

/// Primary keyspace: key = username (UTF-8 bytes), value = msgpack blob.
pub(crate) const USERS_KEYSPACE: &str = "users_v1";
/// Secondary index keyspace: key = user_id (16 bytes), value = username
/// (UTF-8 bytes). Maintained in lock-step with `USERS_KEYSPACE` writes
/// via a fjall `OwnedWriteBatch` so reads from either side see a
/// consistent snapshot.
pub(crate) const USER_ID_INDEX_KEYSPACE: &str = "user_id_to_name_v1";
/// Tertiary index keyspace: key = principal64 projection (8 bytes,
/// big-endian u64), value = username (UTF-8 bytes). Maintained in
/// lock-step with `USERS_KEYSPACE`/`USER_ID_INDEX_KEYSPACE` via the same
/// `OwnedWriteBatch` so all three keyspaces stay consistent. Built once
/// at `open()` via boot-time normalization for pre-existing records, then
/// maintained incrementally by `insert()`/`remove()`.
pub(crate) const PRINCIPAL64_INDEX_KEYSPACE: &str = "principal64_to_name_v1";

// ----------------------------------------------------------------------------
// Persisted blob format
// ----------------------------------------------------------------------------

/// Serializable mirror of [`KdfParams`] (the upstream type does not derive
/// `Serialize` / `Deserialize` because it only travels on the wire as raw
/// bytes inside `auth_message`).
#[derive(Serialize, Deserialize)]
pub(crate) struct PersistedKdfParams {
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
}

impl From<&KdfParams> for PersistedKdfParams {
    fn from(p: &KdfParams) -> Self {
        Self {
            memory_kb: p.memory_kb,
            time: p.time,
            parallelism: p.parallelism,
            argon2_version: p.argon2_version,
        }
    }
}

/// On-disk representation of one user (msgpack-encoded as the value).
///
/// `serde_bytes` keeps the fixed-size byte arrays as compact `bin` types in
/// MessagePack instead of arrays-of-u8, which would otherwise inflate the
/// blob ~3x.
///
/// `pub(crate)` so boot-time normalization migration logic and the
/// fail-closed / `#[serde(default)]`-backward-compat paths can be exercised
/// by in-crate tests (`src/tests/`).
#[derive(Serialize, Deserialize)]
pub(crate) struct PersistedUser {
    #[serde(with = "serde_bytes")]
    pub(crate) user_id: Vec<u8>, // 16 bytes
    #[serde(with = "serde_bytes")]
    pub(crate) salt: Vec<u8>, // 16 bytes
    #[serde(with = "serde_bytes")]
    pub(crate) stored_key: Vec<u8>, // 32 bytes
    #[serde(with = "serde_bytes")]
    pub(crate) server_key: Vec<u8>, // 32 bytes
    pub(crate) kdf_params: PersistedKdfParams,
    // `pub(crate)` so boot-migration / serde-backward-compat tests can read
    // the migrated state. Production code mutates via `from_record` /
    // `read_modify_write`.
    pub(crate) roles: Vec<String>,
    pub(crate) tickets_invalid_before_ns: u64,
    /// Re-encoded from the legacy `"superuser"` role string by boot-time
    /// normalization (§"Boot-time normalization" above). `#[serde(default)]`
    /// so OLD persisted blobs (pre-#556) that lack this field deserialize as
    /// `false` — normalization then fixes any account whose role list still
    /// has the string. Enforcement/mutation of this field (a `SetSuperuser`
    /// wire op, `SessionPermissions` wiring) is task #557's scope, NOT this
    /// task's — this task only adds the field and the one-time re-encoding.
    /// Migrated into the flag below by boot-time normalization.
    #[serde(default)]
    pub(crate) superuser: bool,
    /// Authoritative replication-API capability flag (task #621, mirrors
    /// `superuser` above). Unlike `superuser`, `"replicator"` was NEVER a
    /// valid persisted role string before this task, so there is no
    /// migration to run — `#[serde(default)]` covers every pre-#621 blob
    /// (they deserialize with `replicator == false`), and the string is
    /// simply reserved from this point on (see `update_roles`). No
    /// last-remaining guard and no counter: zero replicators is a normal
    /// state, unlike zero superusers.
    #[serde(default)]
    pub(crate) replicator: bool,
    /// Database-scope for owner-delegation (`authorize_user_lifecycle`).
    /// Task #559: the only Store-B datum with live enforcement meaning
    /// moves *schema-wise* to the directory record. `#[serde(default)]`
    /// — no pre-#559 persisted blob has this field. Set only via
    /// `UserAdminPort::create_user`'s `database` parameter; NOT
    /// auto-imported from shamir-db's retired Store B (design doc §6.3 —
    /// importing risks silently RE-GRANTING a stale scoped-admin privilege
    /// that was never actually enforceable before this task).
    #[serde(default)]
    pub(crate) database: Option<String>,
}

impl PersistedUser {
    pub(crate) fn from_record(
        user_id: [u8; 16],
        record: &UserRecord,
        roles: Vec<String>,
        superuser: bool,
        database: Option<String>,
    ) -> Self {
        Self {
            user_id: user_id.to_vec(),
            salt: record.salt.to_vec(),
            stored_key: record.stored_key.0.to_vec(),
            server_key: record.server_key.as_slice().to_vec(),
            kdf_params: (&record.kdf_params).into(),
            roles,
            tickets_invalid_before_ns: record.tickets_invalid_before_ns,
            superuser,
            replicator: false,
            database,
        }
    }

    pub(crate) fn to_record(&self) -> Option<UserRecord> {
        if self.salt.len() != 16 || self.stored_key.len() != 32 || self.server_key.len() != 32 {
            return None;
        }
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&self.salt);

        let mut stored = [0u8; 32];
        stored.copy_from_slice(&self.stored_key);

        let mut server_key_buf = Zeroizing::new([0u8; 32]);
        server_key_buf.copy_from_slice(&self.server_key);

        Some(UserRecord {
            salt,
            stored_key: StoredKey(stored),
            server_key: server_key_buf,
            kdf_params: KdfParams {
                memory_kb: self.kdf_params.memory_kb,
                time: self.kdf_params.time,
                parallelism: self.kdf_params.parallelism,
                argon2_version: self.kdf_params.argon2_version,
            },
            tickets_invalid_before_ns: self.tickets_invalid_before_ns,
        })
    }

    pub(crate) fn user_id_array(&self) -> Option<[u8; 16]> {
        if self.user_id.len() != 16 {
            return None;
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(&self.user_id);
        Some(id)
    }
}

/// One-lookup snapshot of a user's authoritative directory state, keyed by
/// `user_id`. Built for task #558's Ticket-v2 resume rewrite (resume
/// re-verifies against the directory instead of trusting a stale ticket
/// snapshot). The `RedbUserStateLookup` adapter reads
/// `tickets_invalid_before_ns` off this and, crucially, returns `None` for
/// an unknown `user_id` — closing the fail-open bug where a removed/unknown
/// account collapsed to `Some(0)` (treated as "all tickets valid").
#[derive(Debug, Clone)]
pub struct UserDirectoryState {
    /// Username resolved from the `user_id` reverse index.
    pub username: String,
    /// Authoritative role set (post-normalization: the `"superuser"` string
    /// is migrated into [`Self::superuser`]).
    pub roles: Vec<String>,
    /// Migrated flag (task #557 wires enforcement; this task only carries
    /// the value).
    pub superuser: bool,
    /// Authoritative replication-API capability flag (task #621, mirrors
    /// `superuser` above — no migration needed, see `PersistedUser::replicator`).
    pub replicator: bool,
    /// Database scope for owner-delegation (task #559). `None` for global
    /// users. Read by the `PrincipalResolver` impl so
    /// `authorize_user_lifecycle`'s drop-user scope lookup can resolve via
    /// the directory instead of Store B.
    pub database: Option<String>,
    /// `tickets_invalid_before_ns` — the anti-replay epoch.
    pub tickets_invalid_before_ns: u64,
    /// The directory's stable 128-bit id for this principal. Carried here so
    /// callers (`PrincipalResolver::resolve`/`list`) don't need a second
    /// `user_id()` lookup on top of an already-decoded record.
    pub user_id: [u8; 16],
}

// ----------------------------------------------------------------------------
// Boot-time normalization: pure projection step (fail-closed)
// ----------------------------------------------------------------------------

/// Pure projection step of boot-time normalization: map
/// `(user_id → username)` entries to `(principal64 → username)` entries,
/// failing closed on a zero projection (reserved for `OWNER_SYSTEM` /
/// `Actor::System`) or a collision between two distinct usernames projecting
/// to the same 63-bit id.
///
/// Exposed `pub(crate)` so the fail-closed path can be unit-tested in
/// isolation with engineered inputs — real collisions are cryptographically
/// near-impossible, so a deterministic fixture is the only practical way to
/// cover this branch.
pub(crate) fn project_user_ids_to_principal64(
    entries: Vec<([u8; 16], String)>,
) -> StdResult<Vec<(u64, String)>, String> {
    let mut seen = new_fx_map_wc::<u64, String>(entries.len());
    let mut out = Vec::with_capacity(entries.len());
    for (user_id, username) in entries {
        let projected = principal64(user_id);
        if projected == 0 {
            return Err(format!(
                "principal64 projection of user {username:?} is zero \
                 (reserved for OWNER_SYSTEM/Actor::System) — operator action \
                 required (drop/recreate the account)"
            ));
        }
        if let Some(existing) = seen.insert(projected, username.clone()) {
            return Err(format!(
                "principal64 collision: users {existing:?} and {username:?} \
                 project to the same 63-bit id {projected:#x} — operator action \
                 required (drop/recreate one account)"
            ));
        }
        out.push((projected, username));
    }
    Ok(out)
}
