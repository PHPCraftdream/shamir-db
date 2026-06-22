//! Durable [`UserDirectory`] backed by `fjall` (spec §1.2 + §1.3 + §3.5 + §6.2).
//!
//! Each user record is persisted to a fjall keyspace keyed by username.
//! After every accepted write we call `db.persist(PersistMode::SyncAll)`
//! so the on-disk state always reflects what the server has acknowledged
//! (fsync semantics required by spec §3.5 / §6.2).
//!
//! Roles live alongside the SCRAM-only [`UserRecord`] inside the persisted
//! blob — `shamir-connect` does not yet model roles in its snapshot type, so
//! we expose them via a separate [`FjallUserDirectory::lookup_roles`] method.
//!
//! ## Atomicity
//!
//! `insert` updates two keyspaces (username→blob and user_id→username).
//! fjall's `OwnedWriteBatch` (`db.batch()`) commits cross-keyspace ops
//! atomically, so the two indices never disagree.
//!
//! Read-modify-write operations (`update_roles`, `bump_tickets_invalid`)
//! serialise through a single in-process `parking_lot::Mutex` — admin
//! mutations are low-frequency, so the serialisation cost is irrelevant.

use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use parking_lot::Mutex;
use scc::HashMap as SccHashMap;
use serde::{Deserialize, Serialize};
use shamir_collections::THasher;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use zeroize::Zeroizing;

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::error::{Error, Result};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;

/// Primary keyspace: key = username (UTF-8 bytes), value = msgpack blob.
const USERS_KEYSPACE: &str = "users_v1";
/// Secondary index keyspace: key = user_id (16 bytes), value = username
/// (UTF-8 bytes). Maintained in lock-step with `USERS_KEYSPACE` writes
/// via a fjall `OwnedWriteBatch` so reads from either side see a
/// consistent snapshot.
const USER_ID_INDEX_KEYSPACE: &str = "user_id_to_name_v1";

// ----------------------------------------------------------------------------
// Persisted blob format
// ----------------------------------------------------------------------------

/// Serializable mirror of [`KdfParams`] (the upstream type does not derive
/// `Serialize` / `Deserialize` because it only travels on the wire as raw
/// bytes inside `auth_message`).
#[derive(Serialize, Deserialize)]
struct PersistedKdfParams {
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
#[derive(Serialize, Deserialize)]
struct PersistedUser {
    #[serde(with = "serde_bytes")]
    user_id: Vec<u8>, // 16 bytes
    #[serde(with = "serde_bytes")]
    salt: Vec<u8>, // 16 bytes
    #[serde(with = "serde_bytes")]
    stored_key: Vec<u8>, // 32 bytes
    #[serde(with = "serde_bytes")]
    server_key: Vec<u8>, // 32 bytes
    kdf_params: PersistedKdfParams,
    roles: Vec<String>,
    tickets_invalid_before_ns: u64,
}

impl PersistedUser {
    fn from_record(user_id: [u8; 16], record: &UserRecord, roles: Vec<String>) -> Self {
        Self {
            user_id: user_id.to_vec(),
            salt: record.salt.to_vec(),
            stored_key: record.stored_key.0.to_vec(),
            server_key: record.server_key.as_slice().to_vec(),
            kdf_params: (&record.kdf_params).into(),
            roles,
            tickets_invalid_before_ns: record.tickets_invalid_before_ns,
        }
    }

    fn to_record(&self) -> Option<UserRecord> {
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

    fn user_id_array(&self) -> Option<[u8; 16]> {
        if self.user_id.len() != 16 {
            return None;
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(&self.user_id);
        Some(id)
    }
}

// ----------------------------------------------------------------------------
// FjallUserDirectory
// ----------------------------------------------------------------------------

/// Durable, fjall-backed [`UserDirectory`] implementation.
///
/// All mutating operations (`insert`, `update_roles`, `bump_tickets_invalid`)
/// call `db.persist(PersistMode::SyncAll)` so the journal is fsync'd before
/// the call returns (spec §3.5 / §6.2 NORMATIVE).
pub struct FjallUserDirectory {
    db: Arc<Database>,
    users: Keyspace,
    user_id_index: Keyspace,
    /// Serialises read-modify-write paths so two concurrent admin ops
    /// targeting the same user cannot lose an update.
    write_lock: Mutex<()>,
    /// In-memory authoritative cache of `tickets_invalid_before_ns` keyed by
    /// `user_id` (16 bytes). Warmed once at `open` from the durable store and
    /// updated on every write that changes the field. This eliminates the
    /// per-request double-fjall-get + msgpack decode that the hot-path
    /// §7.5 validity check used to incur.
    ///
    /// SECURITY: this cache is the **authoritative** source for
    /// `tickets_invalid_before_ns_by_user_id`. A stale entry would cause a
    /// revoked ticket to be accepted. Correctness rests on:
    ///   1. Warm-all at `open` (every durable user is loaded).
    ///   2. Update-on-insert (value 0 for new users).
    ///   3. Update inside `read_modify_write` after successful persist.
    ///
    /// A cold miss means "unknown user" → return 0 (the existing fail-open
    /// default).
    tickets_cache: SccHashMap<[u8; 16], AtomicU64, THasher>,
}

impl core::fmt::Debug for FjallUserDirectory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FjallUserDirectory")
            .field("db", &"<fjall::Database>")
            .finish()
    }
}

impl FjallUserDirectory {
    /// Open or create the database at `path`.
    ///
    /// On first use the keyspaces are created. Subsequent opens reuse the
    /// existing data — user records survive crash/restart.
    pub fn open(path: impl AsRef<Path>) -> std::result::Result<Self, fjall::Error> {
        let db = Database::builder(path.as_ref()).open()?;
        let users = db.keyspace(USERS_KEYSPACE, KeyspaceCreateOptions::default)?;
        let user_id_index = db.keyspace(USER_ID_INDEX_KEYSPACE, KeyspaceCreateOptions::default)?;

        let tickets_cache: SccHashMap<[u8; 16], AtomicU64, THasher> =
            SccHashMap::with_hasher(THasher::default());

        // Warm the cache: iterate every durable user record ONCE and populate
        // user_id → tickets_invalid_before_ns. This is O(N) at startup and
        // makes the cache authoritative — a cold miss in the hot path means
        // "unknown user", not "unloaded user".
        for entry in users.iter() {
            let (_key, blob) = match entry.into_inner() {
                Ok(pair) => pair,
                Err(_) => continue,
            };
            if let Ok(user) = rmp_serde::from_slice::<PersistedUser>(blob.as_ref()) {
                if let Some(id) = user.user_id_array() {
                    let _ =
                        tickets_cache.insert(id, AtomicU64::new(user.tickets_invalid_before_ns));
                }
            }
        }

        Ok(Self {
            db: Arc::new(db),
            users,
            user_id_index,
            write_lock: Mutex::new(()),
            tickets_cache,
        })
    }

    /// `tickets_invalid_before_ns` lookup keyed by `user_id` — used by the
    /// connection orchestration layer's `dispatch_request_view` validity
    /// check (spec §7.5: bumped sessions die on the next request).
    ///
    /// `0` is returned both when the user is unknown AND when the field has
    /// never been bumped — both are treated as "no invalidation" by the
    /// caller, so a fail-open default is safe.
    pub fn tickets_invalid_before_ns_by_user_id(&self, user_id: &[u8; 16]) -> u64 {
        // O(1) cache read — no fjall gets, no msgpack decode.
        //
        // The cache is warmed with ALL users at startup and updated on every
        // insert + every read_modify_write, so a miss means "unknown user".
        // Returning 0 matches the prior fail-open behaviour for unknown users
        // (0 = no invalidation).
        self.tickets_cache
            .read(user_id, |_, v| v.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Roles live alongside the SCRAM record but are NOT part of
    /// [`UserRecord`] (shamir-connect's snapshot type is SCRAM-only).
    /// Session-creation code looks them up here.
    pub fn lookup_roles(&self, username: &str) -> Result<Option<Vec<String>>> {
        let blob = match self.read_blob(username)? {
            Some(b) => b,
            None => return Ok(None),
        };
        let user: PersistedUser = rmp_serde::from_slice(&blob)
            .map_err(|e| Error::Encoding(format!("user_dir: decode PersistedUser: {e}")))?;
        Ok(Some(user.roles))
    }

    fn read_blob(&self, username: &str) -> Result<Option<Vec<u8>>> {
        let entry = self
            .users
            .get(username.as_bytes())
            .map_err(|e| Error::Encoding(format!("user_dir keyspace.get: {e}")))?;
        Ok(entry.map(|slice| slice.as_ref().to_vec()))
    }

    fn fresh_user_id() -> [u8; 16] {
        shamir_connect::common::crypto::random_array::<16>()
    }

    /// Update the in-memory cache after a successful durable write.
    ///
    /// - If the entry exists (warm path), the `AtomicU64` is updated in
    ///   place — no allocation, no lock contention.
    /// - If absent (should only happen for a brand-new insert), a new
    ///   `AtomicU64` is inserted.
    ///
    /// This is called under `write_lock`, so only one writer per process
    /// mutates the cache at a time; concurrent readers use `Relaxed` loads
    /// which is safe because the `AtomicU64` is the only mutable state.
    fn update_cache(&self, user_id: &[u8; 16], tickets_invalid_before_ns: u64) {
        if let Some(v) = self.tickets_cache.get(user_id) {
            v.store(tickets_invalid_before_ns, Ordering::Relaxed);
        } else {
            // Insert returns false if the key was already present (race with
            // another writer), but under write_lock only one writer mutates
            // the cache at a time, so this branch always succeeds for a
            // genuinely new user_id.
            let _ = self
                .tickets_cache
                .insert(*user_id, AtomicU64::new(tickets_invalid_before_ns));
        }
    }

    /// Read-modify-write helper. Serialises through `write_lock` so two
    /// concurrent calls for the same user cannot lose an update; persists
    /// (fsync) before returning when a write was made.
    fn read_modify_write<F>(&self, username: &str, mutate: F) -> Result<bool>
    where
        F: FnOnce(&mut PersistedUser) -> bool,
    {
        let _guard = self.write_lock.lock();

        let prior = self
            .users
            .get(username.as_bytes())
            .map_err(|e| Error::Encoding(format!("fjall: get: {e}")))?;
        let blob: Vec<u8> = match prior {
            Some(v) => v.as_ref().to_vec(),
            None => return Err(Error::InvalidInput("user not found")),
        };
        let mut user: PersistedUser = rmp_serde::from_slice(&blob)
            .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;

        let changed = mutate(&mut user);
        if !changed {
            return Ok(false);
        }

        let new_bytes = rmp_serde::to_vec_named(&user)
            .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;
        self.users
            .insert(username.as_bytes(), new_bytes.as_slice())
            .map_err(|e| Error::Encoding(format!("fjall: insert: {e}")))?;

        // Spec §3.5 / §6.2: fsync before returning.
        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        // Update the in-memory cache so the new tickets_invalid_before_ns is
        // visible on the very next hot-path lookup. This covers ALL
        // read_modify_write callers (update_roles + bump_tickets_invalid +
        // any future write) in one place.
        if let Some(id) = user.user_id_array() {
            self.update_cache(&id, user.tickets_invalid_before_ns);
        }
        Ok(true)
    }
}

// ----------------------------------------------------------------------------
// UserDirectory impl
// ----------------------------------------------------------------------------

impl UserDirectory for FjallUserDirectory {
    fn lookup_by_name(&self, username: &str) -> Option<UserRecord> {
        let blob = self.read_blob(username).ok().flatten()?;
        let user: PersistedUser = rmp_serde::from_slice(&blob).ok()?;
        user.to_record()
    }

    fn insert(&self, username: String, record: UserRecord) -> Result<[u8; 16]> {
        let user_id = Self::fresh_user_id();
        // Roles are NOT supplied through this trait method (shamir-connect
        // doesn't model them in `UserRecord`). New entries start with an
        // empty role set; callers that need roles invoke `update_roles`
        // immediately after `insert`. This matches the in-memory reference
        // impl semantics.
        let persisted = PersistedUser::from_record(user_id, &record, Vec::new());
        let bytes = rmp_serde::to_vec_named(&persisted)
            .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;

        let _guard = self.write_lock.lock();

        let exists = self
            .users
            .contains_key(username.as_bytes())
            .map_err(|e| Error::Encoding(format!("fjall: contains_key: {e}")))?;
        if exists {
            return Err(Error::InvalidInput("username exists"));
        }

        // Atomic cross-keyspace write via fjall batch: the username->blob
        // and user_id->username updates land together or not at all.
        let mut batch = self.db.batch();
        batch.insert(&self.users, username.as_bytes(), bytes.as_slice());
        batch.insert(&self.user_id_index, &user_id[..], username.as_bytes());
        batch
            .commit()
            .map_err(|e| Error::Encoding(format!("fjall: batch commit: {e}")))?;

        // Spec §3.5 / §6.2: fsync before returning.
        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        // Populate the cache so the hot-path lookup sees this user immediately.
        // New users start at tickets_invalid_before_ns = 0 (no invalidation).
        self.update_cache(&user_id, persisted.tickets_invalid_before_ns);

        Ok(user_id)
    }

    fn update_roles(&self, username: &str, roles: Vec<String>, now_ns: u64) -> Result<bool> {
        self.read_modify_write(username, |user| {
            let roles_changed = user.roles != roles;
            // Spec §12.6: changing roles must bump `tickets_invalid_before_ns`
            // so existing sessions can no longer use the stale permission cache.
            let ts_changed = now_ns > user.tickets_invalid_before_ns;

            if !roles_changed && !ts_changed {
                return false;
            }
            if roles_changed {
                user.roles = roles;
            }
            if ts_changed {
                user.tickets_invalid_before_ns = now_ns;
            }
            true
        })
    }

    fn bump_tickets_invalid(&self, username: &str, now_ns: u64) -> Result<bool> {
        self.read_modify_write(username, |user| {
            // Monotonic — never go backwards.
            if now_ns <= user.tickets_invalid_before_ns {
                return false;
            }
            user.tickets_invalid_before_ns = now_ns;
            true
        })
    }

    fn user_id(&self, username: &str) -> Option<[u8; 16]> {
        let blob = self.read_blob(username).ok().flatten()?;
        let user: PersistedUser = rmp_serde::from_slice(&blob).ok()?;
        user.user_id_array()
    }
}
