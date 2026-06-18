//! Server-side `Session` struct + concurrent in-memory store.
//!
//! Per spec §7.2 / §7.5. A `Session` is created on:
//! - Successful initial SCRAM auth (server emits `auth_ok`)
//! - Successful resumption (server emits `resume_ok`)
//!
//! Each session carries permissions snapshot + binding info needed for
//! anti-downgrade resumption checks. The store is a `DashMap` keyed by
//! `session_id` (bearer token).

use crate::common::time::{ns, UnixNanos};
use crate::common::types::{limits, BindingMode, TransportKind};
use dashmap::DashMap;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Snapshot of user permissions taken at session creation (spec §7.3).
///
/// Snapshot semantics: changing roles via `updateUser` does **not** mutate
/// existing sessions — it sets `tickets_invalid_before_ns` so the next
/// per-request validity check kills them (spec §7.5).
#[derive(Debug, Clone)]
pub struct SessionPermissions {
    /// Whether this session has the `superuser` role (spec §12 admin commands).
    pub is_superuser: bool,
    /// Roles snapshot.
    pub roles: Vec<String>,
}

impl SessionPermissions {
    /// Construct from a list of roles. `is_superuser` is true iff "superuser"
    /// is among them.
    pub fn from_roles(roles: Vec<String>) -> Self {
        let is_superuser = roles.iter().any(|r| r == "superuser");
        Self {
            is_superuser,
            roles,
        }
    }
}

/// Pending `changePassword` challenge state (spec §12.5).
///
/// Single in-flight per session: the second `changePasswordChallenge`
/// invalidates this. Expires after `CHANGEPW_CHALLENGE_TTL_NS` (5 min).
#[derive(Clone)]
pub struct PendingChangePwChallenge {
    /// Server-side fresh nonce.
    pub server_nonce_cp: [u8; 32],
    /// Client-supplied nonce (anti-replay both directions).
    pub client_nonce_cp: [u8; 32],
    /// When this challenge was issued.
    pub issued_at_ns: u64,
}

impl core::fmt::Debug for PendingChangePwChallenge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PendingChangePwChallenge")
            .field("server_nonce_cp", &"<REDACTED:32>")
            .field("client_nonce_cp", &"<REDACTED:32>")
            .field("issued_at_ns", &self.issued_at_ns)
            .finish()
    }
}

impl core::fmt::Debug for Session {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Session")
            .field("user_id", &"<REDACTED:16>")
            .field("username", &"<REDACTED>")
            .field("permissions", &"<RwLock>")
            .field("created_at_ns", &self.created_at_ns)
            .field("transport_kind", &self.transport_kind)
            .field("binding_mode", &self.binding_mode)
            .field("channel_binding_at_auth", &"<REDACTED:32>")
            .field("pending_changepw_challenge", &"<Mutex>")
            .finish()
    }
}

/// Per-session state held by the server in memory (spec §7.2).
///
/// Custom [`Debug`] impl redacts `channel_binding_at_auth` (TLS exporter
/// derivative — would link logs to a TLS session secret per IMPL §4).
pub struct Session {
    /// Bearer session identifier (same value used as the key in
    /// [`SessionStore`]). Stamped by the store at `insert` time —
    /// `Session::new` initialises it to zeros. Available on the
    /// `&Session` reference passed to request handlers so they
    /// can derive the per-session HMAC key for destructive-op
    /// validation without consulting the store.
    pub session_id: [u8; limits::SESSION_ID_BYTES],
    /// Cached HMAC key derived from `session_id`. Populated by
    /// `SessionStore::insert` right after the session_id is
    /// stamped (or lazily on first `hmac_key()` call). Avoids
    /// the per-op SHA-256 redo on every destructive op in a
    /// batch.
    hmac_key_cache: std::sync::OnceLock<[u8; 32]>,
    /// Stable user identifier.
    pub user_id: [u8; 16],
    /// Username (post-NFC, UsernameCaseMapped).
    pub username: String,
    /// Permissions snapshot at auth time. Frozen for the session's
    /// lifetime — no code path writes to it (verified via repo-wide
    /// grep). The previous `parking_lot::RwLock<...>` wrapper added a
    /// shared-lock acquire on every request just to read a `bool`;
    /// since there's no writer to race with, the lock is dead weight.
    pub permissions: SessionPermissions,
    /// Wall-clock creation timestamp (for spec §7.5 validity check).
    pub created_at_ns: u64,
    /// Last activity wall-clock (idle TTL eviction).
    pub last_activity_ns: AtomicU64,
    /// Transport tag at session creation.
    pub transport_kind: TransportKind,
    /// Binding mode at session creation.
    pub binding_mode: BindingMode,
    /// `tls_exporter_or_zeros` snapshotted at handshake — used by
    /// `changePassword` for `auth_message_cp` and by future ticket bindings.
    pub channel_binding_at_auth: [u8; 32],
    /// In-flight changePassword challenge state.
    pub pending_changepw_challenge: parking_lot::Mutex<Option<PendingChangePwChallenge>>,
}

impl Session {
    /// Construct a new session at the given wall-clock time.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_id: [u8; 16],
        username: String,
        permissions: SessionPermissions,
        transport_kind: TransportKind,
        binding_mode: BindingMode,
        channel_binding_at_auth: [u8; 32],
        now_ns: u64,
    ) -> Self {
        Self {
            session_id: [0u8; limits::SESSION_ID_BYTES],
            hmac_key_cache: std::sync::OnceLock::new(),
            user_id,
            username,
            permissions,
            created_at_ns: now_ns,
            last_activity_ns: AtomicU64::new(now_ns),
            transport_kind,
            binding_mode,
            channel_binding_at_auth,
            pending_changepw_challenge: parking_lot::Mutex::new(None),
        }
    }

    /// Derive the per-session HMAC key from `session_id`. Pure
    /// `SHA256(domain || session_id)` — same shape `hmac_key()`
    /// returns, factored out so `SessionStore::insert` can warm
    /// the cache immediately after stamping `session_id`.
    fn derive_hmac_key(session_id: &[u8; limits::SESSION_ID_BYTES]) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"shamir-db hmac key v1\0");
        h.update(session_id);
        let out = h.finalize();
        let mut k = [0u8; 32];
        k.copy_from_slice(&out);
        k
    }

    /// Per-session HMAC key for destructive-op confirmation.
    ///
    /// Derived purely from `session_id` via a domain-separated
    /// SHA-256, so a JS / native client that has the bearer token
    /// can compute the same key without any extra wire field.
    ///
    /// This is NOT a TLS or auth secret — anyone holding the
    /// bearer token can already act on the session, so deriving
    /// the HMAC key from it adds zero authentication strength.
    /// What it DOES provide is a "deliberate construction" proof:
    /// the client could not have produced the tag without thinking
    /// about each specific drop/clear op. That's the formal
    /// guardrail we want on destructive DDL.
    pub fn hmac_key(&self) -> [u8; 32] {
        *self
            .hmac_key_cache
            .get_or_init(|| Self::derive_hmac_key(&self.session_id))
    }

    /// Update `last_activity_ns` to current wall-clock.
    ///
    /// Calls `UnixNanos::now()` internally — on Windows this is a syscall
    /// (~100 ns). Hot-path callers processing many requests per batch should
    /// use [`Session::touch_at`] instead and reuse a single timestamp
    /// captured by the transport layer.
    pub fn touch(&self) {
        self.touch_at(UnixNanos::now().as_u64());
    }

    /// **Optim #5:** update `last_activity_ns` with a caller-supplied timestamp.
    ///
    /// Allows the transport layer to capture `UnixNanos::now()` once per
    /// request (or per batch) and reuse it across multiple session touches,
    /// amortizing the syscall cost. On Windows this saves ~100 ns/dispatch.
    #[inline]
    pub fn touch_at(&self, now_ns: u64) {
        self.last_activity_ns.store(now_ns, Ordering::Relaxed);
    }

    /// Whether this session has expired by wall-clock.
    pub fn is_expired(&self, now_ns: u64, max_age_ns: u64, idle_ttl_ns: u64) -> bool {
        let last = self.last_activity_ns.load(Ordering::Relaxed);
        now_ns > self.created_at_ns + max_age_ns || now_ns > last + idle_ttl_ns
    }

    /// Derive a stable `u64` principal id from the session's username.
    ///
    /// This is the id used for [`Actor::User(id)`] in the Shomer access
    /// fabric. Deterministic: the same username always maps to the same
    /// `u64`, so `chown`/`chgrp` DDL owner ids are consistent with the
    /// wire principal. `fxhash::hash64` is used because it is fast,
    /// collision-resistant for short username strings, and already a
    /// dependency of this crate (used by `SessionStore`'s hasher).
    /// Id `0` is reserved for `Actor::System`; the hash output is
    /// non-zero for any non-empty input, so no collision with System.
    pub fn principal_id(&self) -> u64 {
        // Mask to 63 bits so the id always fits an i64: the catalogue stores
        // integers as i64 (owner / group-member ids round-trip through
        // InnerValue→msgpack), and a u64 above i64::MAX would be lost on
        // read-back. 63 bits of fxhash is ample for principal identity.
        fxhash::hash64(&self.username) & (i64::MAX as u64)
    }

    /// Per-request session validity check per spec §7.5 [NORMATIVE].
    ///
    /// Returns `false` if `created_at_ns <= tickets_invalid_before_ns` —
    /// caller MUST kick the session immediately and emit `session_invalidated`.
    #[inline]
    pub fn is_valid_for_user(&self, tickets_invalid_before_ns: u64) -> bool {
        self.created_at_ns > tickets_invalid_before_ns
    }
}

/// Spec §7.4 NORMATIVE: hard cap on concurrent sessions per user.
/// Excess sessions are evicted LRU (oldest `last_activity_ns` first).
pub const MAX_SESSIONS_PER_USER: usize = 16;

/// Spec §7.8 NORMATIVE: 5-second grace window after disconnect during
/// which the session is held in `Disconnected` state and may be resumed
/// without ticket refresh. Past the window, eviction is permanent.
pub const DISCONNECT_GRACE_NS: u64 = 5 * ns::SECOND;

/// Concurrent session store — keyed by `session_id` (bearer token).
///
/// `Arc<Session>` so handlers can hold a session reference while admin
/// `kickSession` removes the entry from the map without breaking in-flight
/// processing. Per spec §7: not persistent (server restart drops all sessions).
/// `FxHasher` over the 32-byte session_id is enough — the keys are random
/// bytes from a CSPRNG, DoS resistance comes from rate limiting upstream,
/// not from the hash function. SipHash's per-byte cost is wasted here.
type SessionIdHasher = std::hash::BuildHasherDefault<fxhash::FxHasher>;

#[derive(Debug)]
/// Active sessions indexed by session ID.
pub struct SessionStore {
    by_sid: DashMap<[u8; limits::SESSION_ID_BYTES], Arc<Session>, SessionIdHasher>,
    cap_lock: Mutex<()>,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStore {
    /// Empty store.
    pub fn new() -> Self {
        Self {
            by_sid: DashMap::with_hasher(SessionIdHasher::default()),
            cap_lock: Mutex::new(()),
        }
    }

    /// Insert a fresh session under its `session_id`.
    pub fn insert(
        &self,
        session_id: [u8; limits::SESSION_ID_BYTES],
        mut session: Session,
    ) -> Arc<Session> {
        session.session_id = session_id;
        // Warm the HMAC key cache while we still own the Session
        // (OnceLock::set succeeds on the first attempt). The derive
        // is SHA-256 over 23+32 bytes — cheap to do once at auth
        // time, free for every subsequent destructive-op check.
        let _ = session
            .hmac_key_cache
            .set(Session::derive_hmac_key(&session_id));
        let arc = Arc::new(session);
        self.by_sid.insert(session_id, Arc::clone(&arc));
        arc
    }

    /// **v1 #7:** insert with `MAX_SESSIONS_PER_USER` enforcement.
    ///
    /// If the user already has `max_sessions_per_user` live sessions, the
    /// oldest one (by `last_activity_ns`) is evicted before the new
    /// session is added — spec §7.4 NORMATIVE LRU policy. Returns
    /// `(new_arc, evicted_sid)` where `evicted_sid` is `Some(_)` iff an
    /// LRU eviction happened (caller can emit
    /// `session_evicted{reason="max_sessions_lru"}` per IMPL §3.2).
    pub fn insert_with_per_user_cap(
        &self,
        session_id: [u8; limits::SESSION_ID_BYTES],
        mut session: Session,
        max_sessions_per_user: usize,
    ) -> (Arc<Session>, Option<[u8; limits::SESSION_ID_BYTES]>) {
        session.session_id = session_id;
        let user_id = session.user_id;

        let _cap_guard = self.cap_lock.lock();

        // Atomically: collect snapshot of all current sids for this user
        // (cheap — typically ≤16) and evict LRU if cap reached.
        let evicted = if max_sessions_per_user > 0 {
            let mut user_sids: Vec<([u8; limits::SESSION_ID_BYTES], u64)> = Vec::new();
            for entry in self.by_sid.iter() {
                if entry.value().user_id == user_id {
                    let last = entry.value().last_activity_ns.load(Ordering::Relaxed);
                    user_sids.push((*entry.key(), last));
                }
            }
            if user_sids.len() >= max_sessions_per_user {
                // Evict the LRU (smallest last_activity_ns).
                user_sids.sort_by_key(|(_, last)| *last);
                let victim = user_sids[0].0;
                self.by_sid.remove(&victim);
                Some(victim)
            } else {
                None
            }
        } else {
            None
        };

        let arc = Arc::new(session);
        self.by_sid.insert(session_id, arc.clone());
        (arc, evicted)
    }

    /// Look up a session by id, touching `last_activity_ns` if found.
    ///
    /// Calls `UnixNanos::now()` internally LAZILY (only on hit). On miss
    /// returns immediately without consulting the clock. Hot-path callers
    /// should still prefer [`SessionStore::lookup_at`] to share one
    /// timestamp across multiple session touches.
    pub fn lookup(&self, sid: &[u8; limits::SESSION_ID_BYTES]) -> Option<Arc<Session>> {
        let session = self.by_sid.get(sid).map(|r| r.clone())?;
        session.touch();
        Some(session)
    }

    /// **Optim #5:** lookup with a caller-supplied `now_ns`, avoiding a
    /// `UnixNanos::now()` call inside the hot path.
    pub fn lookup_at(
        &self,
        sid: &[u8; limits::SESSION_ID_BYTES],
        now_ns: u64,
    ) -> Option<Arc<Session>> {
        let session = self.by_sid.get(sid).map(|r| r.clone())?;
        session.touch_at(now_ns);
        Some(session)
    }

    /// Remove a session by id. Returns the previous session if any.
    pub fn remove(&self, sid: &[u8; limits::SESSION_ID_BYTES]) -> Option<Arc<Session>> {
        self.by_sid.remove(sid).map(|(_, v)| v)
    }

    /// Number of currently-live sessions.
    pub fn len(&self) -> usize {
        self.by_sid.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.by_sid.is_empty()
    }

    /// Background GC: remove sessions where `is_expired(now, max_age, idle_ttl)`.
    /// Returns the number of evicted sessions.
    pub fn gc_expired(&self, now_ns: u64, max_age_ns: u64, idle_ttl_ns: u64) -> usize {
        let mut victims: Vec<[u8; limits::SESSION_ID_BYTES]> = Vec::new();
        for entry in self.by_sid.iter() {
            if entry.value().is_expired(now_ns, max_age_ns, idle_ttl_ns) {
                victims.push(*entry.key());
            }
        }
        let removed = victims.len();
        for sid in victims {
            self.by_sid.remove(&sid);
        }
        removed
    }

    /// Snapshot all sessions belonging to a given `user_id` (for admin
    /// `kickSession` / `updateUser` snapshot+kill semantics, spec §12.4 / §12.6).
    pub fn snapshot_by_user(&self, user_id: &[u8; 16]) -> Vec<[u8; limits::SESSION_ID_BYTES]> {
        self.by_sid
            .iter()
            .filter(|e| &e.value().user_id == user_id)
            .map(|e| *e.key())
            .collect()
    }

    /// Number of sessions currently held for a user.
    pub fn count_for_user(&self, user_id: &[u8; 16]) -> usize {
        self.by_sid
            .iter()
            .filter(|e| &e.value().user_id == user_id)
            .count()
    }

    /// Iterate over all (sid, session) pairs — for monitoring/`listSessions`.
    pub fn for_each(&self, mut f: impl FnMut(&[u8; limits::SESSION_ID_BYTES], &Session)) {
        for entry in self.by_sid.iter() {
            f(entry.key(), entry.value());
        }
    }
}
