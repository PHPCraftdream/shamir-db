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
//! `insert`/`remove` update three keyspaces (username→blob,
//! user_id→username, principal64→username). fjall's `OwnedWriteBatch`
//! (`db.batch()`) commits cross-keyspace ops atomically, so the three
//! indices never disagree.
//!
//! Read-modify-write operations (`update_roles`, `bump_tickets_invalid`)
//! serialise through a single in-process `parking_lot::Mutex` — admin
//! mutations are low-frequency, so the serialisation cost is irrelevant.
//!
//! ## Boot-time normalization (design §6 item 2, task #556)
//!
//! Every `open()` re-derives the `principal64` index from the immutable
//! `user_id` index and re-encodes any record still carrying the legacy
//! `"superuser"` role string into the new `superuser: bool` flag. Both
//! steps are idempotent (a pure re-projection / a `contains` check that is
//! simply false on already-migrated data). The projection step fails
//! `open()` CLOSED on a zero projection (reserved for `OWNER_SYSTEM`) or a
//! collision between two distinct usernames — see
//! [`project_user_ids_to_principal64`].

use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use parking_lot::Mutex;
use scc::HashMap as SccHashMap;
use serde::{Deserialize, Serialize};
use shamir_collections::{new_fx_map_wc, THasher};
use shamir_types::access::principal64;
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
/// Tertiary index keyspace: key = principal64 projection (8 bytes,
/// big-endian u64), value = username (UTF-8 bytes). Maintained in
/// lock-step with `USERS_KEYSPACE`/`USER_ID_INDEX_KEYSPACE` via the same
/// `OwnedWriteBatch` so all three keyspaces stay consistent. Built once
/// at `open()` via boot-time normalization for pre-existing records, then
/// maintained incrementally by `insert()`/`remove()`.
const PRINCIPAL64_INDEX_KEYSPACE: &str = "principal64_to_name_v1";

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
///
/// `pub(crate)` so boot-time normalization migration logic and the
/// fail-closed / `#[serde(default)]`-backward-compat paths can be exercised
/// by in-crate tests (`src/tests/`).
#[derive(Serialize, Deserialize)]
pub(crate) struct PersistedUser {
    #[serde(with = "serde_bytes")]
    user_id: Vec<u8>, // 16 bytes
    #[serde(with = "serde_bytes")]
    salt: Vec<u8>, // 16 bytes
    #[serde(with = "serde_bytes")]
    stored_key: Vec<u8>, // 32 bytes
    #[serde(with = "serde_bytes")]
    server_key: Vec<u8>, // 32 bytes
    kdf_params: PersistedKdfParams,
    // `pub(crate)` so boot-migration / serde-backward-compat tests can read
    // the migrated state. Production code mutates via `from_record` /
    // `read_modify_write`.
    pub(crate) roles: Vec<String>,
    tickets_invalid_before_ns: u64,
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
// FjallUserDirectory
// ----------------------------------------------------------------------------

/// Durable, fjall-backed [`UserDirectory`] implementation.
///
/// All mutating operations (`insert`, `remove`, `update_roles`,
/// `bump_tickets_invalid`) call `db.persist(PersistMode::SyncAll)` so the
/// journal is fsync'd before the call returns (spec §3.5 / §6.2 NORMATIVE).
pub struct FjallUserDirectory {
    db: Arc<Database>,
    users: Keyspace,
    user_id_index: Keyspace,
    principal64_index: Keyspace,
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
    ///   4. Evict-on-`remove` (a stale entry surviving removal would resolve
    ///      a deleted account's user_id to a stale tib — reopening the exact
    ///      fail-open bug §"Fix the fail-open UserStateLookup adapter" closes
    ///      via a different path).
    ///
    /// A cold miss means "unknown user" → return 0 (the existing fail-open
    /// default for the §7.5 check; resume's `UserStateLookup` distinguishes
    /// unknown from known via [`FjallUserDirectory::state_by_user_id`]).
    tickets_cache: SccHashMap<[u8; 16], AtomicU64, THasher>,
    /// Count of persisted users with `superuser == true`, warmed once at
    /// `open()` and maintained by `remove()`. Guards the last-superuser
    /// removal in [`FjallUserDirectory::remove`]. `Relaxed` ordering is safe:
    /// the only mutators are `remove()` (under `write_lock`) and the
    /// single-threaded `open()` warm.
    superuser_count: AtomicU64,
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
    /// existing data — user records survive crash/restart — and re-run
    /// boot-time normalization (see the module-level doc), which is
    /// idempotent.
    pub fn open(path: impl AsRef<Path>) -> std::result::Result<Self, fjall::Error> {
        let db = Database::builder(path.as_ref()).open()?;
        let users = db.keyspace(USERS_KEYSPACE, KeyspaceCreateOptions::default)?;
        let user_id_index = db.keyspace(USER_ID_INDEX_KEYSPACE, KeyspaceCreateOptions::default)?;
        let principal64_index =
            db.keyspace(PRINCIPAL64_INDEX_KEYSPACE, KeyspaceCreateOptions::default)?;

        // ---- Boot-time normalization (design §6 item 2) ----
        // Step 1: build the principal64 index from the (immutable) user_id
        // index. Pure re-projection every boot — fail-closed on a zero
        // projection (reserved for OWNER_SYSTEM) or a collision between two
        // distinct usernames. No "only if empty" guard: a partial/interrupted
        // prior boot must still re-derive correctly.
        let p64_entries = Self::collect_principal64_entries(&user_id_index)?;
        let mut p64_batch = db.batch();
        for (projected, username) in &p64_entries {
            let key = projected.to_be_bytes();
            p64_batch.insert(&principal64_index, &key[..], username.as_bytes());
        }
        p64_batch.commit()?;

        // Steps 2 + 3 + cache warm: a single pass over `users` that migrates
        // the legacy "superuser" role string into the flag, warms the
        // tickets cache, and counts superusers.
        let tickets_cache: SccHashMap<[u8; 16], AtomicU64, THasher> =
            SccHashMap::with_hasher(THasher::default());
        let superuser_count = Self::migrate_roles_and_warm(&db, &users, &tickets_cache)?;

        Ok(Self {
            db: Arc::new(db),
            users,
            user_id_index,
            principal64_index,
            write_lock: Mutex::new(()),
            tickets_cache,
            superuser_count: AtomicU64::new(superuser_count),
        })
    }

    /// Collect `(user_id → username)` pairs from the reverse index and
    /// project them through [`principal64`], failing `open()` closed on a
    /// zero projection or collision (see [`project_user_ids_to_principal64`]).
    fn collect_principal64_entries(
        user_id_index: &Keyspace,
    ) -> std::result::Result<Vec<(u64, String)>, fjall::Error> {
        let mut entries: Vec<([u8; 16], String)> = Vec::new();
        for guard in user_id_index.iter() {
            let (k, v) = match guard.into_inner() {
                Ok(pair) => pair,
                Err(_) => continue,
            };
            if k.len() != 16 {
                continue;
            }
            let mut id = [0u8; 16];
            id.copy_from_slice(&k);
            let username = String::from_utf8_lossy(v.as_ref()).into_owned();
            entries.push((id, username));
        }
        project_user_ids_to_principal64(entries).map_err(|msg| {
            fjall::Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
        })
    }

    /// Single-pass boot step: for every persisted user, (a) re-encode the
    /// legacy `"superuser"` role string into the `superuser` flag if still
    /// present (idempotent), (b) warm `tickets_cache`, (c) count superusers.
    /// Migrated blobs land in one atomic batch + fsync.
    fn migrate_roles_and_warm(
        db: &Database,
        users: &Keyspace,
        tickets_cache: &SccHashMap<[u8; 16], AtomicU64, THasher>,
    ) -> std::result::Result<u64, fjall::Error> {
        let mut migrate_batch = db.batch();
        let mut migrations = 0u32;
        let mut superuser_count = 0u64;

        for guard in users.iter() {
            let (username_bytes, blob) = match guard.into_inner() {
                Ok(pair) => pair,
                Err(_) => continue,
            };
            let mut user: PersistedUser = match rmp_serde::from_slice(blob.as_ref()) {
                Ok(u) => u,
                Err(_) => continue,
            };

            // Migrate the legacy "superuser" role string → flag. Idempotent:
            // an already-migrated record has no "superuser" string, so this
            // `any` check is false on the second boot.
            if user.roles.iter().any(|r| r == "superuser") {
                user.roles.retain(|r| r != "superuser");
                user.superuser = true;
                let new_bytes = rmp_serde::to_vec_named(&user).map_err(|e| {
                    fjall::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("rmp encode (superuser migration): {e}"),
                    ))
                })?;
                migrate_batch.insert(users, username_bytes.as_ref(), new_bytes.as_slice());
                migrations += 1;
            }

            if let Some(id) = user.user_id_array() {
                let _ =
                    tickets_cache.insert_sync(id, AtomicU64::new(user.tickets_invalid_before_ns));
            }
            if user.superuser {
                superuser_count += 1;
            }
        }

        if migrations > 0 {
            migrate_batch.commit()?;
            // Spec §3.5 / §6.2: fsync the re-encoding before returning.
            db.persist(PersistMode::SyncAll)?;
        }

        Ok(superuser_count)
    }

    /// `tickets_invalid_before_ns` lookup keyed by `user_id` — used by the
    /// connection orchestration layer's `dispatch_request_view` validity
    /// check (spec §7.5: bumped sessions die on the next request).
    ///
    /// `0` is returned both when the user is unknown AND when the field has
    /// never been bumped — both are treated as "no invalidation" by the
    /// caller, so a fail-open default is safe for THIS hot path. Resume
    /// (`UserStateLookup`) must instead use [`Self::state_by_user_id`] to
    /// distinguish "unknown" from "known-but-zero".
    pub fn tickets_invalid_before_ns_by_user_id(&self, user_id: &[u8; 16]) -> u64 {
        // O(1) cache read — no fjall gets, no msgpack decode.
        //
        // The cache is warmed with ALL users at startup and updated on every
        // insert + every read_modify_write + evicted on remove, so a miss
        // means "unknown user". Returning 0 matches the prior fail-open
        // behaviour for unknown users (0 = no invalidation).
        self.tickets_cache
            .read_sync(user_id, |_, v| v.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Roles live alongside the SCRAM record but are NOT part of
    /// [`UserRecord`] (shamir-connect's snapshot type is SCRAM-only).
    /// Session-creation code looks them up here.
    ///
    /// **Transitional compat (task #556 → #557):** boot-time normalization
    /// migrates the legacy `"superuser"` role *string* into the dedicated
    /// `superuser: bool` flag and removes the string from the persisted
    /// blob. But `SessionPermissions::from_roles` (in `shamir-connect`)
    /// still derives `is_superuser` by scanning the role *strings* — that
    /// wiring is task #557's scope. Until #557 lands, this method
    /// synthesises the `"superuser"` string back into the returned list
    /// when the flag is set, so the session layer sees the effective role
    /// set without any change on its side. This is purely additive: if
    /// the string is already present (e.g. set via `update_roles` before
    /// normalization runs), it is not duplicated.
    pub fn lookup_roles(&self, username: &str) -> Result<Option<Vec<String>>> {
        let blob = match self.read_blob(username)? {
            Some(b) => b,
            None => return Ok(None),
        };
        let user: PersistedUser = rmp_serde::from_slice(&blob)
            .map_err(|e| Error::Encoding(format!("user_dir: decode PersistedUser: {e}")))?;
        let mut roles = user.roles;
        if user.superuser && !roles.iter().any(|r| r == "superuser") {
            roles.push("superuser".to_string());
        }
        Ok(Some(roles))
    }

    fn read_blob(&self, username: &str) -> Result<Option<Vec<u8>>> {
        let entry = self
            .users
            .get(username.as_bytes())
            .map_err(|e| Error::Encoding(format!("user_dir keyspace.get: {e}")))?;
        Ok(entry.map(|slice| slice.as_ref().to_vec()))
    }

    /// Reverse-index read: `user_id` → `username` via `USER_ID_INDEX_KEYSPACE`.
    /// Distinct from the [`UserDirectory::user_id`] forward lookup
    /// (username→user_id via decoding the user's OWN blob); this reads the
    /// `user_id_index` keyspace directly. Returns `None` if the `user_id` is
    /// not found (unknown/removed account).
    fn read_username_by_user_id(&self, user_id: &[u8; 16]) -> Option<String> {
        let entry = self.user_id_index.get(user_id).ok().flatten()?;
        let username = String::from_utf8(entry.as_ref().to_vec()).ok()?;
        Some(username)
    }

    /// One-lookup snapshot of a user's authoritative state, keyed by
    /// `user_id` — built for task #558's Ticket-v2 resume rewrite (resume
    /// re-verifies against the directory instead of trusting a stale ticket
    /// snapshot). Returns `None` if the `user_id` is not found
    /// (unknown/removed account) — this `None` is what closes the resume
    /// fail-open bug (an unknown user used to resolve to `Some(0)`).
    pub fn state_by_user_id(&self, user_id: &[u8; 16]) -> Option<UserDirectoryState> {
        let username = self.read_username_by_user_id(user_id)?;
        let blob = self.read_blob(&username).ok().flatten()?;
        let user: PersistedUser = rmp_serde::from_slice(&blob).ok()?;
        Some(UserDirectoryState {
            username,
            roles: user.roles,
            superuser: user.superuser,
            replicator: user.replicator,
            database: user.database,
            tickets_invalid_before_ns: user.tickets_invalid_before_ns,
            user_id: *user_id,
        })
    }

    fn fresh_user_id() -> [u8; 16] {
        shamir_connect::common::crypto::random_array::<16>()
    }

    /// Mint a `user_id` whose 63-bit [`principal64`] projection is non-zero
    /// (zero is reserved for `OWNER_SYSTEM`/`Actor::System`) and not already
    /// taken by an existing account. The full 128-bit `user_id` is
    /// cryptographically unique on its own, but the 63-bit PROJECTION is
    /// birthday-bound across many accounts, so the collision probe runs on
    /// the projection.
    ///
    /// Called under `write_lock` (by `insert`) so the contains_key probe and
    /// the subsequent batch commit cannot race a concurrent insert.
    fn mint_unique_user_id(&self) -> Result<[u8; 16]> {
        // 2^-63-per-attempt event; generous headroom, not expected to loop.
        const MAX_ATTEMPTS: u32 = 16;
        for _ in 0..MAX_ATTEMPTS {
            let user_id = Self::fresh_user_id();
            let projected = principal64(user_id);
            if projected == 0 {
                continue; // reserved for OWNER_SYSTEM/Actor::System — re-mint
            }
            let key = projected.to_be_bytes();
            let taken = self
                .principal64_index
                .contains_key(&key[..])
                .map_err(|e| Error::Encoding(format!("fjall: contains_key: {e}")))?;
            if !taken {
                return Ok(user_id);
            }
        }
        Err(Error::Encoding(
            "principal64 mint: exhausted retry budget (this should be \
             cryptographically near-impossible — investigate RNG health)"
                .to_string(),
        ))
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
        if let Some(v) = self.tickets_cache.get_sync(user_id) {
            v.store(tickets_invalid_before_ns, Ordering::Relaxed);
        } else {
            // Insert returns false if the key was already present (race with
            // another writer), but under write_lock only one writer mutates
            // the cache at a time, so this branch always succeeds for a
            // genuinely new user_id.
            let _ = self
                .tickets_cache
                .insert_sync(*user_id, AtomicU64::new(tickets_invalid_before_ns));
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

    /// Permanently delete a user account: all three keyspaces, atomically.
    ///
    /// Does NOT evict live sessions itself — `FjallUserDirectory` has no
    /// handle to a `SessionStore`. Per the existing pattern in
    /// `crates/shamir-connect/src/server/admin.rs` (`snapshot_by_user` then
    /// kill, already used by the role-update/credential-change paths), the
    /// CALLER is responsible for snapshotting and killing live sessions for
    /// this `user_id` after a successful `remove()` — this is out of this
    /// task's scope (the wire-level `DropUser` handler wiring is task #559's
    /// job).
    ///
    /// Refuses to remove the last remaining superuser account
    /// (last-superuser guard) — returns a typed error, deletes nothing.
    ///
    /// Returns `Ok(false)` for an already-absent username (idempotent no-op),
    /// `Ok(true)` if the account was removed.
    pub fn remove(&self, username: &str) -> Result<bool> {
        let _guard = self.write_lock.lock();

        let blob = match self.read_blob(username)? {
            Some(b) => b,
            None => return Ok(false), // already absent — idempotent no-op
        };
        let user: PersistedUser = rmp_serde::from_slice(&blob)
            .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;
        let user_id = user
            .user_id_array()
            .ok_or_else(|| Error::Encoding("corrupt user_id in persisted record".to_string()))?;

        if user.superuser && self.superuser_count.load(Ordering::Relaxed) <= 1 {
            return Err(Error::InvalidInput(
                "cannot remove the last remaining superuser account",
            ));
        }

        let projected = principal64(user_id);
        let pkey = projected.to_be_bytes();

        let mut batch = self.db.batch();
        batch.remove(&self.users, username.as_bytes());
        batch.remove(&self.user_id_index, &user_id[..]);
        batch.remove(&self.principal64_index, &pkey[..]);
        batch
            .commit()
            .map_err(|e| Error::Encoding(format!("fjall: batch commit: {e}")))?;

        // Spec §3.5 / §6.2: fsync before returning.
        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        // Evict the tickets_cache entry too — the cache is the AUTHORITATIVE
        // source `tickets_invalid_before_ns_by_user_id` reads from. A stale
        // cache entry surviving past `remove()` would make a deleted
        // account's user_id still resolve to a (stale) tib value instead of
        // the fail-open 0 — reproducing exactly the fail-open bug the
        // `UserStateLookup` fix closes, just via a different path. This is
        // NOT optional.
        let _ = self.tickets_cache.remove_sync(&user_id);

        if user.superuser {
            self.superuser_count.fetch_sub(1, Ordering::Relaxed);
        }

        Ok(true)
    }

    /// Grant or revoke superuser status (task #557). Idempotent (no-op if
    /// the flag is already at the requested value, returns `Ok(false)` with
    /// no write). On an actual change: bumps `tickets_invalid_before_ns`
    /// (spec §12.6 — a privilege change must invalidate existing sessions,
    /// same rule as `update_roles`), persists with `SyncAll`, updates the
    /// `tickets_cache`, and adjusts `superuser_count` atomically in the
    /// SAME critical section as the blob mutation.
    ///
    /// Refuses to revoke the LAST remaining superuser (uses the O(1)
    /// `superuser_count` warmed at `open()` and maintained by `remove()` +
    /// this method) — mirrors `remove()`'s last-superuser guard so the
    /// system can never lock itself out of admin.
    ///
    /// This is deliberately a bespoke method (not routed through
    /// `read_modify_write`) because it needs to adjust `superuser_count`
    /// in the SAME critical section as the blob mutation — `read_modify_write`'s
    /// closure has no way to touch the counter.
    pub fn set_superuser(&self, username: &str, on: bool, now_ns: u64) -> Result<bool> {
        let _guard = self.write_lock.lock();

        let blob = match self.read_blob(username)? {
            Some(b) => b,
            None => return Err(Error::InvalidInput("user not found")),
        };
        let mut user: PersistedUser = rmp_serde::from_slice(&blob)
            .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;

        if user.superuser == on {
            return Ok(false); // already at the requested state — no-op
        }
        // Last-superuser guard: refuse to revoke if this is the only one.
        if user.superuser && !on && self.superuser_count.load(Ordering::Relaxed) <= 1 {
            return Err(Error::InvalidInput(
                "cannot revoke superuser status from the last remaining superuser account",
            ));
        }

        user.superuser = on;
        // Spec §12.6: privilege change must invalidate existing sessions.
        if now_ns > user.tickets_invalid_before_ns {
            user.tickets_invalid_before_ns = now_ns;
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

        if let Some(id) = user.user_id_array() {
            self.update_cache(&id, user.tickets_invalid_before_ns);
        }
        if on {
            self.superuser_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.superuser_count.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(true)
    }

    /// Grant or revoke replication-API access (task #621). Mirrors
    /// [`Self::set_superuser`] almost literally — same idempotent no-op
    /// shape, same §12.6 privilege-change tickets-invalidation bump, same
    /// cache update — but deliberately WITHOUT a last-remaining guard or a
    /// counter: unlike superuser (which must always have at least one
    /// holder so the system can't lock itself out of admin), zero
    /// replicators is a perfectly normal state — nothing is gated on "how
    /// many are left".
    pub fn set_replicator(&self, username: &str, on: bool, now_ns: u64) -> Result<bool> {
        let _guard = self.write_lock.lock();

        let blob = match self.read_blob(username)? {
            Some(b) => b,
            None => return Err(Error::InvalidInput("user not found")),
        };
        let mut user: PersistedUser = rmp_serde::from_slice(&blob)
            .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;

        if user.replicator == on {
            return Ok(false); // already at the requested state — no-op
        }

        user.replicator = on;
        // Spec §12.6-style: privilege change must invalidate existing sessions.
        if now_ns > user.tickets_invalid_before_ns {
            user.tickets_invalid_before_ns = now_ns;
        }
        let new_bytes = rmp_serde::to_vec_named(&user)
            .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;
        self.users
            .insert(username.as_bytes(), new_bytes.as_slice())
            .map_err(|e| Error::Encoding(format!("fjall: insert: {e}")))?;
        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        if let Some(id) = user.user_id_array() {
            self.update_cache(&id, user.tickets_invalid_before_ns);
        }
        Ok(true)
    }

    // ------------------------------------------------------------------
    // Task #559: directory methods backing the PrincipalResolver /
    // UserAdminPort seam. These live on `FjallUserDirectory` itself (not
    // just the adapter) so they share the directory's `write_lock`
    // serialisation and fjall atomicity guarantees — the adapter stays a
    // thin passthrough.
    // ------------------------------------------------------------------

    /// Insert a fresh user carrying a `database` scope, parallel to the
    /// [`UserDirectory::insert`] trait method (which cannot carry
    /// `database` — `shamir-connect`'s `UserRecord` is SCRAM-only). Used
    /// by the `UserAdminPort::create_user` impl so the scope is set
    /// atomically with creation. Roles start empty here; the port attaches
    /// them via `update_roles` (which enforces the `"superuser"` string
    /// reservation).
    pub fn insert_with_scope(
        &self,
        username: String,
        record: UserRecord,
        database: Option<String>,
    ) -> Result<[u8; 16]> {
        let _guard = self.write_lock.lock();

        let exists = self
            .users
            .contains_key(username.as_bytes())
            .map_err(|e| Error::Encoding(format!("fjall: contains_key: {e}")))?;
        if exists {
            return Err(Error::InvalidInput("username exists"));
        }

        let user_id = self.mint_unique_user_id()?;
        let projected = principal64(user_id);
        let pkey = projected.to_be_bytes();

        let persisted = PersistedUser::from_record(user_id, &record, Vec::new(), false, database);
        let bytes = rmp_serde::to_vec_named(&persisted)
            .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;

        let mut batch = self.db.batch();
        batch.insert(&self.users, username.as_bytes(), bytes.as_slice());
        batch.insert(&self.user_id_index, &user_id[..], username.as_bytes());
        batch.insert(&self.principal64_index, &pkey[..], username.as_bytes());
        batch
            .commit()
            .map_err(|e| Error::Encoding(format!("fjall: batch commit: {e}")))?;

        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        self.update_cache(&user_id, persisted.tickets_invalid_before_ns);

        Ok(user_id)
    }

    /// Resolve a principal by its `principal64` projection key, mirroring
    /// [`Self::state_by_user_id`]'s shape/error conventions but keyed by
    /// the projection via the `principal64_to_name_v1` keyspace (built in
    /// #556 specifically for this consumer). Returns `None` for an
    /// unknown/removed projection.
    pub fn resolve_by_principal64(&self, principal64_key: u64) -> Option<UserDirectoryState> {
        let key = principal64_key.to_be_bytes();
        let entry = self.principal64_index.get(&key[..]).ok().flatten()?;
        let username = String::from_utf8(entry.as_ref().to_vec()).ok()?;
        let blob = self.read_blob(&username).ok().flatten()?;
        let user: PersistedUser = rmp_serde::from_slice(&blob).ok()?;
        let user_id = user.user_id_array()?;
        Some(UserDirectoryState {
            username,
            roles: user.roles,
            superuser: user.superuser,
            replicator: user.replicator,
            database: user.database,
            tickets_invalid_before_ns: user.tickets_invalid_before_ns,
            user_id,
        })
    }

    /// One-shot full-directory scan: iterate `users_v1` once, decoding
    /// every `PersistedUser` into a `(principal64, UserDirectoryState)`
    /// pair. O(N) — acceptable, this mirrors `access_tree`/`List`'s
    /// existing cost model. Backs `PrincipalResolver::list`.
    pub fn list_all(&self) -> Result<Vec<(u64, UserDirectoryState)>> {
        let mut out = Vec::new();
        for guard in self.users.iter() {
            let (username_bytes, blob) = match guard.into_inner() {
                Ok(pair) => pair,
                Err(_) => continue,
            };
            let user: PersistedUser = match rmp_serde::from_slice(blob.as_ref()) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let username = String::from_utf8_lossy(username_bytes.as_ref()).into_owned();
            let Some(uid) = user.user_id_array() else {
                continue;
            };
            let projected = principal64(uid);
            out.push((
                projected,
                UserDirectoryState {
                    username,
                    roles: user.roles,
                    superuser: user.superuser,
                    replicator: user.replicator,
                    database: user.database,
                    tickets_invalid_before_ns: user.tickets_invalid_before_ns,
                    user_id: uid,
                },
            ));
        }
        Ok(out)
    }

    /// Atomically add a role string to a user's role list (no-op if already
    /// present). Mirrors `set_superuser`'s bespoke-method precedent: the
    /// whole read-modify-write runs under `write_lock` so two concurrent
    /// grants to the same user cannot lose an update (the adapter-side
    /// read-then-write alternative would have a TOCTOU gap). Rejects the
    /// reserved `"superuser"` string (use `set_superuser` for the flag) and
    /// bumps `tickets_invalid_before_ns` on a real change (spec §12.6).
    pub fn grant_role(&self, username: &str, role: &str, now_ns: u64) -> Result<bool> {
        if role == "superuser" {
            return Err(Error::InvalidInput(
                "\"superuser\" is a reserved role name — use SetSuperuser to grant/revoke superuser status",
            ));
        }
        self.read_modify_write(username, |user| {
            if user.roles.iter().any(|r| r == role) {
                return false;
            }
            user.roles.push(role.to_string());
            if now_ns > user.tickets_invalid_before_ns {
                user.tickets_invalid_before_ns = now_ns;
            }
            true
        })
    }

    /// Atomically remove a role string from a user's role list (no-op if
    /// absent). See [`Self::grant_role`] for the atomicity rationale.
    pub fn revoke_role(&self, username: &str, role: &str, now_ns: u64) -> Result<bool> {
        self.read_modify_write(username, |user| {
            let before = user.roles.len();
            user.roles.retain(|r| r != role);
            let changed = user.roles.len() != before;
            if changed && now_ns > user.tickets_invalid_before_ns {
                user.tickets_invalid_before_ns = now_ns;
            }
            changed
        })
    }
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
) -> std::result::Result<Vec<(u64, String)>, String> {
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
        let _guard = self.write_lock.lock();

        let exists = self
            .users
            .contains_key(username.as_bytes())
            .map_err(|e| Error::Encoding(format!("fjall: contains_key: {e}")))?;
        if exists {
            return Err(Error::InvalidInput("username exists"));
        }

        // Mint a user_id whose 63-bit principal64 projection is non-zero and
        // not already taken. Under write_lock so the uniqueness probe and the
        // batch commit below cannot race a concurrent insert.
        let user_id = self.mint_unique_user_id()?;
        let projected = principal64(user_id);
        let pkey = projected.to_be_bytes();

        // Roles are NOT supplied through this trait method (shamir-connect
        // doesn't model them in `UserRecord`). New entries start with an
        // empty role set and `superuser = false`; callers that need roles
        // invoke `update_roles` immediately after `insert`. (Creating a NEW
        // superuser via the field is task #557's `SetSuperuser`/bootstrap
        // wiring — NOT this task.) This matches the in-memory reference impl
        // semantics.
        let persisted = PersistedUser::from_record(user_id, &record, Vec::new(), false, None);
        let bytes = rmp_serde::to_vec_named(&persisted)
            .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;

        // Atomic cross-keyspace write via fjall batch: all three indices land
        // together or not at all.
        let mut batch = self.db.batch();
        batch.insert(&self.users, username.as_bytes(), bytes.as_slice());
        batch.insert(&self.user_id_index, &user_id[..], username.as_bytes());
        batch.insert(&self.principal64_index, &pkey[..], username.as_bytes());
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
        // Task #557: reserve the literal `"superuser"` string at this single
        // write boundary — every role-writing caller (create_scram_user's
        // `update_roles`, the future GrantRole port, etc.) goes through
        // here, so this one check closes the reservation for all of them.
        // Superuser status is granted/revoked ONLY via `SetSuperuser` /
        // `set_superuser`, which mutates the dedicated `superuser: bool`
        // flag in the same critical section as `superuser_count`.
        if roles.iter().any(|r| r == "superuser") {
            return Err(Error::InvalidInput(
                "\"superuser\" is a reserved role name — use SetSuperuser to grant/revoke superuser status",
            ));
        }
        // Task #621: reserve "replicator" at the same write boundary,
        // mirroring the "superuser" reservation above — the replication
        // capability is granted/revoked ONLY via `SetReplicator` /
        // `set_replicator`, which mutates the dedicated `replicator: bool`
        // flag.
        if roles.iter().any(|r| r == "replicator") {
            return Err(Error::InvalidInput(
                "\"replicator\" is a reserved role name — use SetReplicator to grant/revoke replication access",
            ));
        }
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

    fn update_credentials(
        &self,
        username: &str,
        new_salt: [u8; 16],
        new_stored_key: StoredKey,
        new_server_key: [u8; 32],
        new_kdf_params: KdfParams,
        now_ns: u64,
    ) -> Result<bool> {
        // Fold the `tickets_invalid_before_ns` bump into the SAME
        // read-modify-write transaction as the credential swap (rather than
        // a second separate `bump_tickets_invalid` call): `changepw.rs`'s doc
        // comment on `finalize_change_password` notes the caller persists
        // `tickets_invalid_before_ns_ns` "for atomicity reasons" — a second,
        // independent fjall write would reopen exactly the gap that note
        // warns about (a crash between the two writes could leave new
        // credentials durable while the old ticket epoch is still honoured,
        // i.e. a stolen ticket minted under the OLD password would still
        // pass the §7.5 validity check after the password change). Doing
        // both mutations under one `write_lock` critical section + one
        // `persist(SyncAll)` makes the update atomic with respect to a
        // crash: either both land or neither does.
        self.read_modify_write(username, |user| {
            user.salt = new_salt.to_vec();
            user.stored_key = new_stored_key.0.to_vec();
            user.server_key = new_server_key.to_vec();
            user.kdf_params = (&new_kdf_params).into();
            if now_ns > user.tickets_invalid_before_ns {
                user.tickets_invalid_before_ns = now_ns;
            }
            true
        })
    }
}
