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
//! - Lockout snapshot (periodic dump of in-memory failure/lockout tables;
//!   read on startup to rehydrate, written every 60s by a background task —
//!   see `server::launch`)
//! - Install / boot timestamps
//!
//! Layout: ONE fjall keyspace `server_meta_v1` mapping `&str` (key name) →
//! `&[u8]` (msgpack-encoded value). Each setter performs an atomic write
//! followed by `db.persist(PersistMode::SyncAll)` so the journal is fsync'd
//! before the call returns (spec IMPL §1.3 / §6.2 NORMATIVE).
//!
//! ## Atomicity
//!
//! fjall has no nested ACID transactions. Read-modify-write setters
//! (rotation, bootstrap consume, etc.) serialise through a single
//! `parking_lot::Mutex` so two concurrent admin ops on the same key
//! cannot lose an update. The mutex covers the entire RMW window plus
//! the fsync — getters are lock-free.

use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use zeroize::Zeroizing;

use shamir_connect::common::crypto::random_bytes;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::bootstrap::BootstrapState;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::lockout::LockoutSnapshot;
use shamir_connect::server::rate_limit::RateLimitSnapshot;
use shamir_connect::server::rotation::ServerIdentityState;

// ---------------------------------------------------------------------------
// Keyspace + key constants
// ---------------------------------------------------------------------------

const META_KEYSPACE: &str = "server_meta_v1";

const KEY_SECRETS: &str = "secrets";
const KEY_AUDIT_CHAIN: &str = "audit_chain";
const KEY_TICKET: &str = "ticket";
const KEY_IDENTITY: &str = "identity";
const KEY_BOOTSTRAP: &str = "bootstrap";
const KEY_AUDIT_CHECKPOINT: &str = "audit_checkpoint";
const KEY_TIMES: &str = "times";
/// Periodic dump of `InMemoryLockoutStore` state (spec IMPL §1.3 — failed
/// auth bookkeeping persisted across restarts so an attacker cannot reset
/// brute-force lockout by inducing a restart).
const KEY_LOCKOUT_SNAPSHOT: &str = "lockout_snapshot";
/// Periodic dump of `InMemoryRateLimiter` token buckets.
const KEY_RATELIMIT_SNAPSHOT: &str = "ratelimit_snapshot";

// ---------------------------------------------------------------------------
// Persisted blobs (one per logical chunk)
// ---------------------------------------------------------------------------

/// Serde glue: (de)serialize secret byte buffers as `serde_bytes` blobs while
/// holding them in `Zeroizing<Vec<u8>>` so the key material is wiped on drop.
mod serde_zeroizing_bytes {
    use serde::{Deserializer, Serializer};
    use zeroize::Zeroizing;

    pub fn serialize<S: Serializer>(v: &Zeroizing<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::serialize(v.as_slice(), s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Zeroizing<Vec<u8>>, D::Error> {
        let v: Vec<u8> = serde_bytes::deserialize(d)?;
        Ok(Zeroizing::new(v))
    }

    pub mod opt {
        use serde::{Deserializer, Serializer};
        use zeroize::Zeroizing;

        pub fn serialize<S: Serializer>(
            v: &Option<Zeroizing<Vec<u8>>>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match v {
                Some(inner) => s.serialize_some(serde_bytes::Bytes::new(inner.as_slice())),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Zeroizing<Vec<u8>>>, D::Error> {
            let v: Option<Vec<u8>> = serde_bytes::deserialize(d)?;
            Ok(v.map(Zeroizing::new))
        }
    }
}

#[derive(Serialize, Deserialize)]
struct PersistedSecrets {
    #[serde(with = "serde_zeroizing_bytes")]
    server_secret: Zeroizing<Vec<u8>>,
    #[serde(with = "serde_zeroizing_bytes::opt")]
    server_secret_previous: Option<Zeroizing<Vec<u8>>>,
    server_secret_rotated_at_ns: u64,
    #[serde(with = "serde_zeroizing_bytes")]
    lockout_secret: Zeroizing<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
struct PersistedAuditChain {
    #[serde(with = "serde_zeroizing_bytes")]
    audit_chain_key: Zeroizing<Vec<u8>>,
    #[serde(with = "serde_zeroizing_bytes::opt")]
    audit_chain_key_previous: Option<Zeroizing<Vec<u8>>>,
    audit_chain_key_rotated_at_ns: u64,
}

#[derive(Serialize, Deserialize)]
struct PersistedTicket {
    #[serde(with = "serde_zeroizing_bytes")]
    ticket_key: Zeroizing<Vec<u8>>,
    #[serde(with = "serde_zeroizing_bytes::opt")]
    ticket_key_previous: Option<Zeroizing<Vec<u8>>>,
    ticket_key_rotated_at_ns: u64,
}

#[derive(Serialize, Deserialize)]
struct PersistedIdentity {
    #[serde(with = "serde_zeroizing_bytes")]
    current_seed: Zeroizing<Vec<u8>>,
    #[serde(with = "serde_zeroizing_bytes::opt")]
    previous_seed: Option<Zeroizing<Vec<u8>>>,
    rotation_until_ns: Option<u64>,
    current_version: u64,
}

#[derive(Serialize, Deserialize)]
struct PersistedBootstrap {
    #[serde(with = "serde_zeroizing_bytes::opt")]
    bootstrap_token_hash: Option<Zeroizing<Vec<u8>>>,
    bootstrap_token_expires_at_ns: Option<u64>,
    superuser_ever_existed: bool,
}

#[derive(Serialize, Deserialize)]
struct PersistedAuditCheckpoint {
    last_audit_seq: u64,
    #[serde(with = "serde_zeroizing_bytes")]
    last_audit_hmac: Zeroizing<Vec<u8>>,
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
    /// fjall keyspace open / get / insert / persist failures.
    #[error("fjall: {0}")]
    Fjall(#[from] fjall::Error),
    /// MessagePack encode / decode failure or length mismatch on a fixed-size
    /// byte field.
    #[error("encoding: {0}")]
    Encoding(String),
    /// Plain I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A required key was absent — the store has not been initialised.
    #[error("not initialised: {0}")]
    NotInitialised(&'static str),
}

// ---------------------------------------------------------------------------
// ServerMetaStore
// ---------------------------------------------------------------------------

/// Durable server-meta store backed by a single fjall keyspace.
///
/// All mutations fsync (`db.persist(PersistMode::SyncAll)`) before
/// returning, per spec IMPLEMENTATION_GUIDE §1.3 / §6.2 NORMATIVE.
pub struct ServerMetaStore {
    db: Arc<Database>,
    meta: Keyspace,
    /// Serialises read-modify-write setters (rotation, bootstrap consume, ...).
    write_lock: Mutex<()>,
}

impl core::fmt::Debug for ServerMetaStore {
    /// Custom debug — redacts ALL key bytes (spec IMPL §4 NORMATIVE).
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ServerMetaStore")
            .field("db", &"<fjall::Database>")
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
    /// material when the keyspace does not yet exist (or has no data yet).
    pub fn open_or_init(path: impl AsRef<Path>) -> Result<Self, MetaError> {
        let db = Database::builder(path.as_ref()).open()?;
        let meta = db.keyspace(META_KEYSPACE, KeyspaceCreateOptions::default)?;
        let store = Self {
            db: Arc::new(db),
            meta,
            write_lock: Mutex::new(()),
        };

        let needs_init = store.meta.get(KEY_SECRETS.as_bytes())?.is_none();
        if needs_init {
            store.write_initial_state()?;
        } else {
            store.touch_last_started_at()?;
        }

        Ok(store)
    }

    /// One-shot durable init.
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
            server_secret: Zeroizing::new(server_secret.to_vec()),
            server_secret_previous: None,
            server_secret_rotated_at_ns: now,
            lockout_secret: Zeroizing::new(lockout_secret.to_vec()),
        };
        let audit = PersistedAuditChain {
            audit_chain_key: Zeroizing::new(audit_chain_key.to_vec()),
            audit_chain_key_previous: None,
            audit_chain_key_rotated_at_ns: now,
        };
        let ticket = PersistedTicket {
            ticket_key: Zeroizing::new(ticket_key.to_vec()),
            ticket_key_previous: None,
            ticket_key_rotated_at_ns: now,
        };
        let identity = PersistedIdentity {
            current_seed: Zeroizing::new(ed25519_seed.to_vec()),
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

        let _guard = self.write_lock.lock();
        self.put(KEY_SECRETS, &secrets)?;
        self.put(KEY_AUDIT_CHAIN, &audit)?;
        self.put(KEY_TICKET, &ticket)?;
        self.put(KEY_IDENTITY, &identity)?;
        self.put(KEY_BOOTSTRAP, &bootstrap)?;
        self.put(KEY_TIMES, &times)?;
        self.persist()
    }

    fn touch_last_started_at(&self) -> Result<(), MetaError> {
        let now = UnixNanos::now().as_u64();
        let _guard = self.write_lock.lock();
        let mut times: PersistedTimes = match self.get(KEY_TIMES)? {
            Some(t) => t,
            None => PersistedTimes {
                created_at_ns: now,
                last_started_at_ns: now,
            },
        };
        times.last_started_at_ns = now;
        self.put(KEY_TIMES, &times)?;
        self.persist()
    }

    // -----------------------------------------------------------------
    // Internal key/value helpers
    // -----------------------------------------------------------------

    fn put<T: Serialize>(&self, key: &str, value: &T) -> Result<(), MetaError> {
        let bytes = rmp_serde::to_vec_named(value)
            .map_err(|e| MetaError::Encoding(format!("rmp encode {key}: {e}")))?;
        self.meta.insert(key.as_bytes(), bytes.as_slice())?;
        Ok(())
    }

    fn get<T>(&self, key: &str) -> Result<Option<T>, MetaError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let entry = match self.meta.get(key.as_bytes())? {
            Some(e) => e,
            None => return Ok(None),
        };
        let v: T = rmp_serde::from_slice(&entry)
            .map_err(|e| MetaError::Encoding(format!("rmp decode {key}: {e}")))?;
        Ok(Some(v))
    }

    fn persist(&self) -> Result<(), MetaError> {
        self.db.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    fn read_blob<T>(&self, key: &str) -> Result<Option<T>, MetaError>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.get(key)
    }

    // -----------------------------------------------------------------
    // Getters (lock-free reads)
    // -----------------------------------------------------------------

    /// Return the current `(server_secret, lockout_secret)` as a clonable
    /// `ServerSecrets` value.
    pub fn server_secrets(&self) -> Result<ServerSecrets, MetaError> {
        let p: PersistedSecrets = self
            .read_blob(KEY_SECRETS)?
            .ok_or(MetaError::NotInitialised(KEY_SECRETS))?;
        Ok(ServerSecrets {
            server_secret: bytes32(&p.server_secret),
            lockout_secret: bytes32(&p.lockout_secret),
        })
    }

    /// Current `audit_chain_key`.
    pub fn audit_chain_key(&self) -> Result<[u8; 32], MetaError> {
        let p: PersistedAuditChain = self
            .read_blob(KEY_AUDIT_CHAIN)?
            .ok_or(MetaError::NotInitialised(KEY_AUDIT_CHAIN))?;
        Ok(bytes32(&p.audit_chain_key))
    }

    /// Current ticket key plus optional previous key (during rotation overlap).
    pub fn ticket_keys(&self) -> Result<([u8; 32], Option<[u8; 32]>), MetaError> {
        let p: PersistedTicket = self
            .read_blob(KEY_TICKET)?
            .ok_or(MetaError::NotInitialised(KEY_TICKET))?;
        let current = bytes32(&p.ticket_key);
        let previous = p.ticket_key_previous.as_ref().map(|v| bytes32(v));
        Ok((current, previous))
    }

    /// Current Ed25519 identity seed (32 bytes).
    pub fn current_identity_seed(&self) -> Result<[u8; 32], MetaError> {
        let p: PersistedIdentity = self
            .read_blob(KEY_IDENTITY)?
            .ok_or(MetaError::NotInitialised(KEY_IDENTITY))?;
        Ok(bytes32(&p.current_seed))
    }

    /// Rehydrated [`ServerIdentityState`].
    pub fn identity_state(&self) -> Result<ServerIdentityState, MetaError> {
        let p: PersistedIdentity = self
            .read_blob(KEY_IDENTITY)?
            .ok_or(MetaError::NotInitialised(KEY_IDENTITY))?;
        let current_seed = bytes32(&p.current_seed);
        let previous_seed = p.previous_seed.as_ref().map(|v| bytes32(v));
        Ok(ServerIdentityState::from_material(
            &current_seed,
            previous_seed.as_ref(),
            p.rotation_until_ns,
            p.current_version,
        ))
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
    // Setters (each acquires `write_lock`, does RMW or pure write, fsyncs)
    // -----------------------------------------------------------------

    /// Move `current → previous`, install `new_key` as current, set
    /// `rotated_at_ns = now_ns`.
    pub fn rotate_ticket_key(&self, new_key: [u8; 32], now_ns: u64) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        let prior: PersistedTicket = self.get(KEY_TICKET)?.ok_or_else(|| missing(KEY_TICKET))?;
        let next = PersistedTicket {
            ticket_key: Zeroizing::new(new_key.to_vec()),
            ticket_key_previous: Some(prior.ticket_key),
            ticket_key_rotated_at_ns: now_ns,
        };
        self.put(KEY_TICKET, &next)?;
        self.persist()
    }

    /// Rotate the audit-chain HMAC key (current → previous, install new).
    pub fn rotate_audit_chain_key(&self, new_key: [u8; 32], now_ns: u64) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        let prior: PersistedAuditChain = self
            .get(KEY_AUDIT_CHAIN)?
            .ok_or_else(|| missing(KEY_AUDIT_CHAIN))?;
        let next = PersistedAuditChain {
            audit_chain_key: Zeroizing::new(new_key.to_vec()),
            audit_chain_key_previous: Some(prior.audit_chain_key),
            audit_chain_key_rotated_at_ns: now_ns,
        };
        self.put(KEY_AUDIT_CHAIN, &next)?;
        self.persist()
    }

    /// Rotate the anti-enumeration `server_secret`. `lockout_secret` is NEVER rotated.
    pub fn rotate_server_secret(&self, new_secret: [u8; 32], now_ns: u64) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        let prior: PersistedSecrets = self.get(KEY_SECRETS)?.ok_or_else(|| missing(KEY_SECRETS))?;
        let next = PersistedSecrets {
            server_secret: Zeroizing::new(new_secret.to_vec()),
            server_secret_previous: Some(prior.server_secret),
            server_secret_rotated_at_ns: now_ns,
            lockout_secret: prior.lockout_secret,
        };
        self.put(KEY_SECRETS, &next)?;
        self.persist()
    }

    /// Persist identity post-`rotate()`.
    pub fn store_identity_after_rotate(
        &self,
        current_seed: [u8; 32],
        previous_seed: [u8; 32],
        rotation_until_ns: u64,
        new_version: u64,
    ) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        let next = PersistedIdentity {
            current_seed: Zeroizing::new(current_seed.to_vec()),
            previous_seed: Some(Zeroizing::new(previous_seed.to_vec())),
            rotation_until_ns: Some(rotation_until_ns),
            current_version: new_version,
        };
        self.put(KEY_IDENTITY, &next)?;
        self.persist()
    }

    /// Background-task callback after the 7-day overlap completes.
    pub fn finalize_identity_rotation(&self) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        let prior: PersistedIdentity = self
            .get(KEY_IDENTITY)?
            .ok_or_else(|| missing(KEY_IDENTITY))?;
        let next = PersistedIdentity {
            current_seed: prior.current_seed,
            previous_seed: None,
            rotation_until_ns: None,
            current_version: prior.current_version,
        };
        self.put(KEY_IDENTITY, &next)?;
        self.persist()
    }

    /// Install a fresh bootstrap-token hash + expiry.
    pub fn set_bootstrap_token(&self, hash: [u8; 32], expires_at_ns: u64) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        let prior: PersistedBootstrap = self.get(KEY_BOOTSTRAP)?.unwrap_or(PersistedBootstrap {
            bootstrap_token_hash: None,
            bootstrap_token_expires_at_ns: None,
            superuser_ever_existed: false,
        });
        let next = PersistedBootstrap {
            bootstrap_token_hash: Some(Zeroizing::new(hash.to_vec())),
            bootstrap_token_expires_at_ns: Some(expires_at_ns),
            superuser_ever_existed: prior.superuser_ever_existed,
        };
        self.put(KEY_BOOTSTRAP, &next)?;
        self.persist()
    }

    /// Atomically clear `bootstrap_token_hash` and set
    /// `superuser_ever_existed = true`. Idempotent.
    pub fn consume_bootstrap_token(&self) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        let prior: PersistedBootstrap = self.get(KEY_BOOTSTRAP)?.unwrap_or(PersistedBootstrap {
            bootstrap_token_hash: None,
            bootstrap_token_expires_at_ns: None,
            superuser_ever_existed: false,
        });
        if prior.bootstrap_token_hash.is_none()
            && prior.bootstrap_token_expires_at_ns.is_none()
            && prior.superuser_ever_existed
        {
            return Ok(());
        }
        let next = PersistedBootstrap {
            bootstrap_token_hash: None,
            bootstrap_token_expires_at_ns: None,
            superuser_ever_existed: true,
        };
        self.put(KEY_BOOTSTRAP, &next)?;
        self.persist()
    }

    /// Persist a fresh audit-checkpoint snapshot.
    pub fn store_audit_checkpoint(
        &self,
        next_seq: u64,
        prev_hmac: [u8; 32],
    ) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        let next = PersistedAuditCheckpoint {
            last_audit_seq: next_seq,
            last_audit_hmac: Zeroizing::new(prev_hmac.to_vec()),
        };
        self.put(KEY_AUDIT_CHECKPOINT, &next)?;
        self.persist()
    }

    /// Load the last persisted lockout snapshot, if any.
    pub fn lockout_snapshot(&self) -> Result<Option<LockoutSnapshot>, MetaError> {
        self.read_blob::<LockoutSnapshot>(KEY_LOCKOUT_SNAPSHOT)
    }

    /// Persist a lockout snapshot.
    pub fn store_lockout_snapshot(&self, snapshot: &LockoutSnapshot) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        self.put(KEY_LOCKOUT_SNAPSHOT, snapshot)?;
        self.persist()
    }

    /// Load the last persisted rate-limit snapshot, if any.
    pub fn ratelimit_snapshot(&self) -> Result<Option<RateLimitSnapshot>, MetaError> {
        self.read_blob::<RateLimitSnapshot>(KEY_RATELIMIT_SNAPSHOT)
    }

    /// Persist a rate-limit snapshot.
    pub fn store_ratelimit_snapshot(&self, snapshot: &RateLimitSnapshot) -> Result<(), MetaError> {
        let _guard = self.write_lock.lock();
        self.put(KEY_RATELIMIT_SNAPSHOT, snapshot)?;
        self.persist()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn missing(key: &str) -> MetaError {
    MetaError::Encoding(format!("server_meta key missing: {key}"))
}

fn bytes32(v: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = v.len().min(32);
    out[..n].copy_from_slice(&v[..n]);
    out
}
