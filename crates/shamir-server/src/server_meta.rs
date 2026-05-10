//! Durable server-meta storage (spec IMPLEMENTATION_GUIDE §1.2 NORMATIVE).
//!
//! Holds the per-server long-lived state:
//! - `server_secret` (anti-enumeration HKDF IKM, rotated)
//! - `lockout_secret` (separate, NEVER rotated)
//! - `audit_chain_key` (with `previous` for rotation overlap)
//! - `ticket_key` (with optional previous + `rotated_at_ns`)
//! - Ed25519 server identity seed (current + optional previous + version)
//! - Bootstrap state (`bootstrap_token_hash`, `superuser_ever_existed`)
//! - Audit checkpoint (`last_audit_seq`, `last_audit_hmac`)
//! - Install / boot timestamps
//!
//! Layout: ONE redb table `server_meta_v1` mapping `&str` (key name) →
//! `&[u8]` (msgpack-encoded value). Each setter performs an atomic write
//! transaction with [`Durability::Immediate`] so the file is fsync'd before
//! the call returns (spec IMPL §1.3 / §6.2 NORMATIVE).

use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

use shamir_connect::common::crypto::random_bytes;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::bootstrap::BootstrapState;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::rotation::ServerIdentityState;

// ---------------------------------------------------------------------------
// Table layout + key constants
// ---------------------------------------------------------------------------

const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("server_meta_v1");

const KEY_SECRETS: &str = "secrets";
const KEY_AUDIT_CHAIN: &str = "audit_chain";
const KEY_TICKET: &str = "ticket";
const KEY_IDENTITY: &str = "identity";
const KEY_BOOTSTRAP: &str = "bootstrap";
const KEY_AUDIT_CHECKPOINT: &str = "audit_checkpoint";
const KEY_TIMES: &str = "times";

// ---------------------------------------------------------------------------
// Persisted blobs (one per logical chunk)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct PersistedSecrets {
    #[serde(with = "serde_bytes")]
    server_secret: Vec<u8>, // 32
    #[serde(with = "serde_bytes")]
    server_secret_previous: Option<Vec<u8>>, // 32
    server_secret_rotated_at_ns: u64,
    #[serde(with = "serde_bytes")]
    lockout_secret: Vec<u8>, // 32 — NEVER rotated
}

#[derive(Serialize, Deserialize)]
struct PersistedAuditChain {
    #[serde(with = "serde_bytes")]
    audit_chain_key: Vec<u8>, // 32
    #[serde(with = "serde_bytes")]
    audit_chain_key_previous: Option<Vec<u8>>, // 32
    audit_chain_key_rotated_at_ns: u64,
}

#[derive(Serialize, Deserialize)]
struct PersistedTicket {
    #[serde(with = "serde_bytes")]
    ticket_key: Vec<u8>, // 32
    #[serde(with = "serde_bytes")]
    ticket_key_previous: Option<Vec<u8>>, // 32
    ticket_key_rotated_at_ns: u64,
}

#[derive(Serialize, Deserialize)]
struct PersistedIdentity {
    /// 32-byte Ed25519 seed (NOT the priv-key — `from_seed` reconstructs it).
    #[serde(with = "serde_bytes")]
    current_seed: Vec<u8>,
    #[serde(with = "serde_bytes")]
    previous_seed: Option<Vec<u8>>,
    rotation_until_ns: Option<u64>,
    current_version: u64,
}

#[derive(Serialize, Deserialize)]
struct PersistedBootstrap {
    #[serde(with = "serde_bytes")]
    bootstrap_token_hash: Option<Vec<u8>>, // 32
    bootstrap_token_expires_at_ns: Option<u64>,
    superuser_ever_existed: bool,
}

#[derive(Serialize, Deserialize)]
struct PersistedAuditCheckpoint {
    last_audit_seq: u64,
    #[serde(with = "serde_bytes")]
    last_audit_hmac: Vec<u8>, // 32
}

#[derive(Serialize, Deserialize)]
struct PersistedTimes {
    created_at_ns: u64,
    last_started_at_ns: u64,
}

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

/// Errors returned by [`ServerMetaStore`].
#[derive(Debug, Error)]
pub enum MetaError {
    /// Generic redb error (covers `Database::create`, `Database::open`,
    /// and rare internal paths through `From<DatabaseError>` /
    /// `From<SetDurabilityError>`).
    #[error("redb: {0}")]
    Redb(#[from] redb::Error),
    /// Failed to begin a transaction.
    #[error("transaction: {0}")]
    Transaction(#[from] redb::TransactionError),
    /// Failed to open / mutate a table.
    #[error("table: {0}")]
    Table(#[from] redb::TableError),
    /// Underlying storage error (read / insert / range / etc.).
    #[error("storage: {0}")]
    Storage(#[from] redb::StorageError),
    /// Commit (fsync) failure.
    #[error("commit: {0}")]
    Commit(#[from] redb::CommitError),
    /// MessagePack encode / decode failure or length mismatch on a fixed-size
    /// byte field.
    #[error("encoding: {0}")]
    Encoding(String),
    /// Plain I/O failure (kept for symmetry with the other stores).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// ServerMetaStore
// ---------------------------------------------------------------------------

/// Durable server-meta store backed by a single redb file.
///
/// All mutations use [`Durability::Immediate`] (fsync) per spec
/// IMPLEMENTATION_GUIDE §1.3 / §6.2 NORMATIVE.
pub struct ServerMetaStore {
    db: Arc<Database>,
}

impl core::fmt::Debug for ServerMetaStore {
    /// Custom debug — redacts ALL key bytes (spec IMPL §4 NORMATIVE).
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ServerMetaStore")
            .field("db", &"<redb::Database>")
            .field("server_secret", &"<REDACTED>")
            .field("lockout_secret", &"<REDACTED>")
            .field("audit_chain_key", &"<REDACTED>")
            .field("ticket_key", &"<REDACTED>")
            .field("ed25519_seed", &"<REDACTED>")
            .field("bootstrap_token_hash", &"<REDACTED>")
            .field("last_audit_hmac", &"<REDACTED>")
            .finish()
    }
}

impl ServerMetaStore {
    /// Open existing store at `path`, or create-and-init with FRESH random
    /// material when the file does not yet exist (or has no data yet).
    ///
    /// Init flow generates `server_secret`, `lockout_secret`, `audit_chain_key`,
    /// `ticket_key`, Ed25519 seed, sets `created_at_ns = now`,
    /// `current_version = 0`. Everything is committed under a single write
    /// transaction with [`Durability::Immediate`].
    ///
    /// `last_started_at_ns` is updated to "now" on every open.
    pub fn open_or_init(path: impl AsRef<Path>) -> Result<Self, MetaError> {
        let db = Database::create(path).map_err(redb::Error::from)?;
        let store = Self { db: Arc::new(db) };

        // Decide whether init is needed. We probe for the secrets blob — if
        // present we treat the file as already initialised. Otherwise we
        // generate fresh material and persist it inside a single durable
        // transaction.
        let needs_init = {
            // Make sure the table exists so the read txn doesn't fail with
            // TableDoesNotExist on a brand-new file.
            let mut wtxn = store.db.begin_write()?;
            wtxn.set_durability(Durability::Immediate)
                .map_err(redb::Error::from)?;
            {
                let _t = wtxn.open_table(META_TABLE)?;
            }
            wtxn.commit()?;

            let rtxn = store.db.begin_read()?;
            let table = rtxn.open_table(META_TABLE)?;
            table.get(KEY_SECRETS)?.is_none()
        };

        if needs_init {
            store.write_initial_state()?;
        } else {
            store.touch_last_started_at()?;
        }

        Ok(store)
    }

    /// One-shot durable init under a single transaction.
    fn write_initial_state(&self) -> Result<(), MetaError> {
        let now = UnixNanos::now().as_u64();

        let mut server_secret = [0u8; 32];
        random_bytes(&mut server_secret);
        let mut lockout_secret = [0u8; 32];
        random_bytes(&mut lockout_secret);
        let mut audit_chain_key = [0u8; 32];
        random_bytes(&mut audit_chain_key);
        let mut ticket_key = [0u8; 32];
        random_bytes(&mut ticket_key);
        let mut ed25519_seed = [0u8; 32];
        random_bytes(&mut ed25519_seed);

        let secrets = PersistedSecrets {
            server_secret: server_secret.to_vec(),
            server_secret_previous: None,
            server_secret_rotated_at_ns: now,
            lockout_secret: lockout_secret.to_vec(),
        };
        let audit = PersistedAuditChain {
            audit_chain_key: audit_chain_key.to_vec(),
            audit_chain_key_previous: None,
            audit_chain_key_rotated_at_ns: now,
        };
        let ticket = PersistedTicket {
            ticket_key: ticket_key.to_vec(),
            ticket_key_previous: None,
            ticket_key_rotated_at_ns: now,
        };
        let identity = PersistedIdentity {
            current_seed: ed25519_seed.to_vec(),
            previous_seed: None,
            rotation_until_ns: None,
            current_version: 0,
        };
        let bootstrap = PersistedBootstrap {
            bootstrap_token_hash: None,
            bootstrap_token_expires_at_ns: None,
            superuser_ever_existed: false,
        };
        let times = PersistedTimes {
            created_at_ns: now,
            last_started_at_ns: now,
        };

        self.with_write_txn(|table| {
            put(table, KEY_SECRETS, &secrets)?;
            put(table, KEY_AUDIT_CHAIN, &audit)?;
            put(table, KEY_TICKET, &ticket)?;
            put(table, KEY_IDENTITY, &identity)?;
            put(table, KEY_BOOTSTRAP, &bootstrap)?;
            put(table, KEY_TIMES, &times)?;
            // Audit checkpoint stays absent until first store_audit_checkpoint.
            Ok(())
        })
    }

    fn touch_last_started_at(&self) -> Result<(), MetaError> {
        let now = UnixNanos::now().as_u64();
        self.with_write_txn(|table| {
            let mut times: PersistedTimes = match get(table, KEY_TIMES)? {
                Some(t) => t,
                None => PersistedTimes {
                    created_at_ns: now,
                    last_started_at_ns: now,
                },
            };
            times.last_started_at_ns = now;
            put(table, KEY_TIMES, &times)?;
            Ok(())
        })
    }

    // -----------------------------------------------------------------
    // Internal write-txn helper
    // -----------------------------------------------------------------

    fn with_write_txn<F, T>(&self, f: F) -> Result<T, MetaError>
    where
        F: FnOnce(&mut redb::Table<'_, &str, &[u8]>) -> Result<T, MetaError>,
    {
        let mut txn = self.db.begin_write()?;
        txn.set_durability(Durability::Immediate)
            .map_err(redb::Error::from)?;
        let result = {
            let mut table = txn.open_table(META_TABLE)?;
            f(&mut table)?
        };
        txn.commit()?;
        Ok(result)
    }

    fn read_blob<T>(&self, key: &str) -> Result<Option<T>, MetaError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(META_TABLE)?;
        let entry = match table.get(key)? {
            Some(e) => e,
            None => return Ok(None),
        };
        let v: T = rmp_serde::from_slice(entry.value())
            .map_err(|e| MetaError::Encoding(format!("rmp decode {key}: {e}")))?;
        Ok(Some(v))
    }

    // -----------------------------------------------------------------
    // Getters (lock-free reads — each opens a fresh redb read txn)
    // -----------------------------------------------------------------

    /// Return the current `(server_secret, lockout_secret)` as a clonable
    /// `ServerSecrets` value.
    pub fn server_secrets(&self) -> ServerSecrets {
        let p: PersistedSecrets = self
            .read_blob(KEY_SECRETS)
            .ok()
            .flatten()
            .expect("server_meta secrets missing — store not initialised");
        ServerSecrets {
            server_secret: bytes32(&p.server_secret),
            lockout_secret: bytes32(&p.lockout_secret),
        }
    }

    /// Current `audit_chain_key`.
    pub fn audit_chain_key(&self) -> [u8; 32] {
        let p: PersistedAuditChain = self
            .read_blob(KEY_AUDIT_CHAIN)
            .ok()
            .flatten()
            .expect("server_meta audit_chain missing — store not initialised");
        bytes32(&p.audit_chain_key)
    }

    /// Current ticket key plus optional previous key (during rotation overlap).
    pub fn ticket_keys(&self) -> ([u8; 32], Option<[u8; 32]>) {
        let p: PersistedTicket = self
            .read_blob(KEY_TICKET)
            .ok()
            .flatten()
            .expect("server_meta ticket missing — store not initialised");
        let current = bytes32(&p.ticket_key);
        let previous = p.ticket_key_previous.as_ref().map(|v| bytes32(v));
        (current, previous)
    }

    /// Current Ed25519 identity seed (32 bytes). Useful for callers that
    /// need a raw [`Ed25519Keypair`] alongside the [`ServerIdentityState`]
    /// — e.g. `connection.rs` holds a separate keypair handle so it can
    /// pass `&Ed25519Keypair` into `verify_proof` (the rotation state
    /// doesn't expose the keypair directly).
    pub fn current_identity_seed(&self) -> [u8; 32] {
        let p: PersistedIdentity = self
            .read_blob(KEY_IDENTITY)
            .ok()
            .flatten()
            .expect("server_meta identity missing — store not initialised");
        bytes32(&p.current_seed)
    }

    /// Rehydrated [`ServerIdentityState`].
    pub fn identity_state(&self) -> ServerIdentityState {
        let p: PersistedIdentity = self
            .read_blob(KEY_IDENTITY)
            .ok()
            .flatten()
            .expect("server_meta identity missing — store not initialised");
        let current_seed = bytes32(&p.current_seed);
        let previous_seed = p.previous_seed.as_ref().map(|v| bytes32(v));
        ServerIdentityState::from_material(
            &current_seed,
            previous_seed.as_ref(),
            p.rotation_until_ns,
            p.current_version,
        )
    }

    /// Rehydrated [`BootstrapState`] (constructed via `from_meta`).
    pub fn bootstrap_state(&self) -> BootstrapState {
        let p: PersistedBootstrap =
            self.read_blob(KEY_BOOTSTRAP)
                .ok()
                .flatten()
                .unwrap_or(PersistedBootstrap {
                    bootstrap_token_hash: None,
                    bootstrap_token_expires_at_ns: None,
                    superuser_ever_existed: false,
                });
        let token_hash = p.bootstrap_token_hash.as_ref().map(|v| bytes32(v));
        BootstrapState::from_meta(
            token_hash,
            p.bootstrap_token_expires_at_ns,
            p.superuser_ever_existed,
        )
    }

    /// Audit checkpoint: `(seq, hmac)` if any has ever been stored.
    pub fn audit_checkpoint(&self) -> Option<(u64, [u8; 32])> {
        let p: PersistedAuditCheckpoint = self.read_blob(KEY_AUDIT_CHECKPOINT).ok().flatten()?;
        Some((p.last_audit_seq, bytes32(&p.last_audit_hmac)))
    }

    /// Server install time (set once on first init).
    pub fn created_at_ns(&self) -> u64 {
        self.read_blob::<PersistedTimes>(KEY_TIMES)
            .ok()
            .flatten()
            .map(|t| t.created_at_ns)
            .unwrap_or(0)
    }

    /// Last successful open / restart timestamp.
    pub fn last_started_at_ns(&self) -> u64 {
        self.read_blob::<PersistedTimes>(KEY_TIMES)
            .ok()
            .flatten()
            .map(|t| t.last_started_at_ns)
            .unwrap_or(0)
    }

    /// True iff the bootstrap-token row currently has a token hash present.
    pub fn bootstrap_token_active(&self) -> bool {
        self.read_blob::<PersistedBootstrap>(KEY_BOOTSTRAP)
            .ok()
            .flatten()
            .and_then(|p| p.bootstrap_token_hash)
            .is_some()
    }

    /// True iff a superuser has EVER been provisioned (sticky flag).
    pub fn superuser_ever_existed(&self) -> bool {
        self.read_blob::<PersistedBootstrap>(KEY_BOOTSTRAP)
            .ok()
            .flatten()
            .map(|p| p.superuser_ever_existed)
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------
    // Setters (each is one durable write transaction)
    // -----------------------------------------------------------------

    /// Move `current → previous`, install `new_key` as current, set
    /// `rotated_at_ns = now_ns`.
    pub fn rotate_ticket_key(&self, new_key: [u8; 32], now_ns: u64) -> Result<(), MetaError> {
        self.with_write_txn(|table| {
            let prior: PersistedTicket =
                get(table, KEY_TICKET)?.ok_or_else(|| missing(KEY_TICKET))?;
            let next = PersistedTicket {
                ticket_key: new_key.to_vec(),
                ticket_key_previous: Some(prior.ticket_key),
                ticket_key_rotated_at_ns: now_ns,
            };
            put(table, KEY_TICKET, &next)
        })
    }

    /// Rotate the audit-chain HMAC key (current → previous, install new).
    pub fn rotate_audit_chain_key(&self, new_key: [u8; 32], now_ns: u64) -> Result<(), MetaError> {
        self.with_write_txn(|table| {
            let prior: PersistedAuditChain =
                get(table, KEY_AUDIT_CHAIN)?.ok_or_else(|| missing(KEY_AUDIT_CHAIN))?;
            let next = PersistedAuditChain {
                audit_chain_key: new_key.to_vec(),
                audit_chain_key_previous: Some(prior.audit_chain_key),
                audit_chain_key_rotated_at_ns: now_ns,
            };
            put(table, KEY_AUDIT_CHAIN, &next)
        })
    }

    /// Rotate the anti-enumeration `server_secret` (current → previous,
    /// install new). `lockout_secret` is NEVER rotated.
    pub fn rotate_server_secret(&self, new_secret: [u8; 32], now_ns: u64) -> Result<(), MetaError> {
        self.with_write_txn(|table| {
            let prior: PersistedSecrets =
                get(table, KEY_SECRETS)?.ok_or_else(|| missing(KEY_SECRETS))?;
            let next = PersistedSecrets {
                server_secret: new_secret.to_vec(),
                server_secret_previous: Some(prior.server_secret),
                server_secret_rotated_at_ns: now_ns,
                lockout_secret: prior.lockout_secret,
            };
            put(table, KEY_SECRETS, &next)
        })
    }

    /// Persist identity post-`rotate()` — atomically stores the new current
    /// seed plus the previous seed and the overlap deadline.
    pub fn store_identity_after_rotate(
        &self,
        current_seed: [u8; 32],
        previous_seed: [u8; 32],
        rotation_until_ns: u64,
        new_version: u64,
    ) -> Result<(), MetaError> {
        self.with_write_txn(|table| {
            let next = PersistedIdentity {
                current_seed: current_seed.to_vec(),
                previous_seed: Some(previous_seed.to_vec()),
                rotation_until_ns: Some(rotation_until_ns),
                current_version: new_version,
            };
            put(table, KEY_IDENTITY, &next)
        })
    }

    /// Background-task callback after the 7-day overlap completes. Clears
    /// `previous_seed` and `rotation_until_ns`, leaves `current_seed` and
    /// `current_version` untouched.
    pub fn finalize_identity_rotation(&self) -> Result<(), MetaError> {
        self.with_write_txn(|table| {
            let prior: PersistedIdentity =
                get(table, KEY_IDENTITY)?.ok_or_else(|| missing(KEY_IDENTITY))?;
            let next = PersistedIdentity {
                current_seed: prior.current_seed,
                previous_seed: None,
                rotation_until_ns: None,
                current_version: prior.current_version,
            };
            put(table, KEY_IDENTITY, &next)
        })
    }

    /// Install a fresh bootstrap-token hash + expiry. Caller validates that
    /// `superuser_ever_existed` is `false` BEFORE invoking (this layer
    /// performs only the write).
    pub fn set_bootstrap_token(&self, hash: [u8; 32], expires_at_ns: u64) -> Result<(), MetaError> {
        self.with_write_txn(|table| {
            let prior: PersistedBootstrap =
                get(table, KEY_BOOTSTRAP)?.unwrap_or(PersistedBootstrap {
                    bootstrap_token_hash: None,
                    bootstrap_token_expires_at_ns: None,
                    superuser_ever_existed: false,
                });
            let next = PersistedBootstrap {
                bootstrap_token_hash: Some(hash.to_vec()),
                bootstrap_token_expires_at_ns: Some(expires_at_ns),
                superuser_ever_existed: prior.superuser_ever_existed,
            };
            put(table, KEY_BOOTSTRAP, &next)
        })
    }

    /// Atomically clear `bootstrap_token_hash` and set
    /// `superuser_ever_existed = true`. Idempotent — calling on an already
    /// consumed state is a no-op (no error).
    pub fn consume_bootstrap_token(&self) -> Result<(), MetaError> {
        self.with_write_txn(|table| {
            let prior: PersistedBootstrap =
                get(table, KEY_BOOTSTRAP)?.unwrap_or(PersistedBootstrap {
                    bootstrap_token_hash: None,
                    bootstrap_token_expires_at_ns: None,
                    superuser_ever_existed: false,
                });
            let next = PersistedBootstrap {
                bootstrap_token_hash: None,
                bootstrap_token_expires_at_ns: None,
                superuser_ever_existed: true,
            };
            // Avoid a useless write if state already terminal.
            if prior.bootstrap_token_hash.is_none()
                && prior.bootstrap_token_expires_at_ns.is_none()
                && prior.superuser_ever_existed
            {
                return Ok(());
            }
            put(table, KEY_BOOTSTRAP, &next)
        })
    }

    /// Persist a fresh audit-checkpoint snapshot. `next_seq` is the seq of
    /// the LAST entry covered by `prev_hmac` (mirror of audit_appender state).
    pub fn store_audit_checkpoint(
        &self,
        next_seq: u64,
        prev_hmac: [u8; 32],
    ) -> Result<(), MetaError> {
        self.with_write_txn(|table| {
            let next = PersistedAuditCheckpoint {
                last_audit_seq: next_seq,
                last_audit_hmac: prev_hmac.to_vec(),
            };
            put(table, KEY_AUDIT_CHECKPOINT, &next)
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn put<T: Serialize>(
    table: &mut redb::Table<'_, &str, &[u8]>,
    key: &str,
    value: &T,
) -> Result<(), MetaError> {
    let bytes = rmp_serde::to_vec_named(value)
        .map_err(|e| MetaError::Encoding(format!("rmp encode {key}: {e}")))?;
    table.insert(key, bytes.as_slice())?;
    Ok(())
}

fn get<T>(table: &redb::Table<'_, &str, &[u8]>, key: &str) -> Result<Option<T>, MetaError>
where
    T: for<'de> Deserialize<'de>,
{
    let entry = match table.get(key)? {
        Some(e) => e,
        None => return Ok(None),
    };
    let v: T = rmp_serde::from_slice(entry.value())
        .map_err(|e| MetaError::Encoding(format!("rmp decode {key}: {e}")))?;
    Ok(Some(v))
}

fn missing(key: &str) -> MetaError {
    MetaError::Encoding(format!("server_meta key missing: {key}"))
}

fn bytes32(v: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = v.len().min(32);
    out[..n].copy_from_slice(&v[..n]);
    out
}
