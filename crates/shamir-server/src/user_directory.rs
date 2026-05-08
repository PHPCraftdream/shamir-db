//! Durable [`UserDirectory`] backed by `redb` (spec §1.2 + §1.3 + §3.5 + §6.2).
//!
//! Each user record is persisted to a single redb table keyed by username.
//! Write transactions commit with [`redb::Durability::Immediate`] so the
//! on-disk state always reflects what the server has acknowledged (fsync
//! semantics required by spec §3.5 / §6.2).
//!
//! Roles live alongside the SCRAM-only [`UserRecord`] inside the persisted
//! blob — `shamir-connect` does not yet model roles in its snapshot type, so
//! we expose them via a separate [`RedbUserDirectory::lookup_roles`] method.
//!
//! Wire-redaction: only the database handle is custom-debugged
//! (`<redb::Database>`); secret-bearing fields live inside [`UserRecord`]
//! which already implements [`core::fmt::Debug`] with redaction.

use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::error::{Error, Result};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;

/// Single redb table: key = username (UTF-8 str), value = msgpack blob.
const USERS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("users_v1");

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

/// On-disk representation of one user (msgpack-encoded as the table value).
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
// RedbUserDirectory
// ----------------------------------------------------------------------------

/// Durable, redb-backed [`UserDirectory`] implementation.
///
/// All mutating operations (`insert`, `update_roles`, `bump_tickets_invalid`)
/// commit with [`Durability::Immediate`] so the file is fsync'd before the
/// call returns (spec §3.5 / §6.2 NORMATIVE).
pub struct RedbUserDirectory {
    db: Arc<Database>,
}

impl core::fmt::Debug for RedbUserDirectory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RedbUserDirectory")
            .field("db", &"<redb::Database>")
            .finish()
    }
}

impl RedbUserDirectory {
    /// Open or create the database file at `path`.
    ///
    /// On first use the table is created. Subsequent opens reuse the
    /// existing data — user records survive crash/restart.
    pub fn open(path: impl AsRef<Path>) -> std::result::Result<Self, redb::Error> {
        let db = Database::create(path)?;
        // Ensure the table exists so later read transactions on a fresh DB
        // don't fail with `TableDoesNotExist`.
        let txn = db.begin_write()?;
        {
            let _t = txn.open_table(USERS_TABLE)?;
        }
        txn.commit()?;
        Ok(Self { db: Arc::new(db) })
    }

    /// Roles live alongside the SCRAM record but are NOT part of
    /// [`UserRecord`] (shamir-connect's snapshot type is SCRAM-only).
    /// Session-creation code looks them up here.
    pub fn lookup_roles(&self, username: &str) -> Option<Vec<String>> {
        let blob = self.read_blob(username)?;
        let user: PersistedUser = rmp_serde::from_slice(&blob).ok()?;
        Some(user.roles)
    }

    fn read_blob(&self, username: &str) -> Option<Vec<u8>> {
        let txn = self.db.begin_read().ok()?;
        let table = txn.open_table(USERS_TABLE).ok()?;
        let entry = table.get(username).ok().flatten()?;
        Some(entry.value().to_vec())
    }

    fn fresh_user_id() -> [u8; 16] {
        // Reuse shamir-connect's CSPRNG wrapper (OsRng) to avoid pulling in
        // a direct `rand` dep at this crate level.
        shamir_connect::common::crypto::random_array::<16>()
    }

    /// Common write-transaction body: `mutate` runs holding the table open
    /// and may load+rewrite the blob; the caller decides what to commit.
    fn write_with<F, T>(&self, mutate: F) -> Result<T>
    where
        F: FnOnce(&mut redb::Table<'_, &str, &[u8]>) -> Result<T>,
    {
        let mut txn = self
            .db
            .begin_write()
            .map_err(|e| Error::Encoding(format!("redb: begin_write: {e}")))?;
        // Spec §3.5 / §6.2: persist durably before returning.
        txn.set_durability(Durability::Immediate)
            .map_err(|e| Error::Encoding(format!("redb: set_durability: {e}")))?;

        let result = {
            let mut table = txn
                .open_table(USERS_TABLE)
                .map_err(|e| Error::Encoding(format!("redb: open_table: {e}")))?;
            mutate(&mut table)?
        };

        txn.commit()
            .map_err(|e| Error::Encoding(format!("redb: commit: {e}")))?;
        Ok(result)
    }
}

// ----------------------------------------------------------------------------
// UserDirectory impl
// ----------------------------------------------------------------------------

impl UserDirectory for RedbUserDirectory {
    fn lookup_by_name(&self, username: &str) -> Option<UserRecord> {
        let blob = self.read_blob(username)?;
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

        self.write_with(|table| {
            // Reject duplicates atomically inside the same write txn. Scope
            // the read so the AccessGuard's borrow on `table` ends before we
            // re-borrow mutably for `insert`.
            let exists = {
                table
                    .get(username.as_str())
                    .map_err(|e| Error::Encoding(format!("redb: get: {e}")))?
                    .is_some()
            };
            if exists {
                return Err(Error::InvalidInput("username exists"));
            }
            table
                .insert(username.as_str(), bytes.as_slice())
                .map_err(|e| Error::Encoding(format!("redb: insert: {e}")))?;
            Ok(())
        })?;

        Ok(user_id)
    }

    fn update_roles(&self, username: &str, roles: Vec<String>, now_ns: u64) -> Result<bool> {
        self.write_with(|table| {
            // Scope the read so the AccessGuard's borrow on `table` ends
            // before we re-borrow mutably for `insert`.
            let blob: Vec<u8> = {
                let prior = table
                    .get(username)
                    .map_err(|e| Error::Encoding(format!("redb: get: {e}")))?;
                match prior {
                    Some(v) => v.value().to_vec(),
                    None => return Err(Error::InvalidInput("user not found")),
                }
            };
            let mut user: PersistedUser = rmp_serde::from_slice(&blob)
                .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;

            let roles_changed = user.roles != roles;
            // Spec §12.6: changing roles must bump `tickets_invalid_before_ns`
            // so existing sessions can no longer use the stale permission cache.
            let ts_changed = now_ns > user.tickets_invalid_before_ns;

            if !roles_changed && !ts_changed {
                return Ok(false);
            }

            if roles_changed {
                user.roles = roles;
            }
            if ts_changed {
                user.tickets_invalid_before_ns = now_ns;
            }

            let new_bytes = rmp_serde::to_vec_named(&user)
                .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;
            table
                .insert(username, new_bytes.as_slice())
                .map_err(|e| Error::Encoding(format!("redb: insert: {e}")))?;
            Ok(true)
        })
    }

    fn bump_tickets_invalid(&self, username: &str, now_ns: u64) -> Result<bool> {
        self.write_with(|table| {
            // Scope the read so the AccessGuard's borrow on `table` ends
            // before we re-borrow mutably for `insert`.
            let blob: Vec<u8> = {
                let prior = table
                    .get(username)
                    .map_err(|e| Error::Encoding(format!("redb: get: {e}")))?;
                match prior {
                    Some(v) => v.value().to_vec(),
                    None => return Err(Error::InvalidInput("user not found")),
                }
            };
            let mut user: PersistedUser = rmp_serde::from_slice(&blob)
                .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;

            // Monotonic — never go backwards.
            if now_ns <= user.tickets_invalid_before_ns {
                return Ok(false);
            }
            user.tickets_invalid_before_ns = now_ns;

            let new_bytes = rmp_serde::to_vec_named(&user)
                .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;
            table
                .insert(username, new_bytes.as_slice())
                .map_err(|e| Error::Encoding(format!("redb: insert: {e}")))?;
            Ok(true)
        })
    }

    fn user_id(&self, username: &str) -> Option<[u8; 16]> {
        let blob = self.read_blob(username)?;
        let user: PersistedUser = rmp_serde::from_slice(&blob).ok()?;
        user.user_id_array()
    }
}
