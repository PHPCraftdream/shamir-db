//! Server-side `Session` struct + concurrent in-memory store.
//!
//! Per spec §7.2 / §7.5. A `Session` is created on:
//! - Successful initial SCRAM auth (server emits `auth_ok`)
//! - Successful resumption (server emits `resume_ok`)
//!
//! Each session carries permissions snapshot + binding info needed for
//! anti-downgrade resumption checks. The store is a `DashMap` keyed by
//! `session_id` (bearer token).

use crate::common::time::UnixNanos;
use crate::common::types::{limits, BindingMode, TransportKind};
use dashmap::DashMap;
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
        Self { is_superuser, roles }
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
    /// Stable user identifier.
    pub user_id: [u8; 16],
    /// Username (post-NFC, UsernameCaseMapped).
    pub username: String,
    /// Permissions snapshot at auth time.
    pub permissions: parking_lot::RwLock<SessionPermissions>,
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
            user_id,
            username,
            permissions: parking_lot::RwLock::new(permissions),
            created_at_ns: now_ns,
            last_activity_ns: AtomicU64::new(now_ns),
            transport_kind,
            binding_mode,
            channel_binding_at_auth,
            pending_changepw_challenge: parking_lot::Mutex::new(None),
        }
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
        now_ns > self.created_at_ns + max_age_ns
            || now_ns > last + idle_ttl_ns
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

/// Concurrent session store — keyed by `session_id` (bearer token).
///
/// `Arc<Session>` so handlers can hold a session reference while admin
/// `kickSession` removes the entry from the map without breaking in-flight
/// processing. Per spec §7: not persistent (server restart drops all sessions).
#[derive(Debug, Default)]
pub struct SessionStore {
    by_sid: DashMap<[u8; limits::SESSION_ID_BYTES], Arc<Session>>,
}

impl SessionStore {
    /// Empty store.
    pub fn new() -> Self {
        Self {
            by_sid: DashMap::new(),
        }
    }

    /// Insert a fresh session under its `session_id`.
    pub fn insert(
        &self,
        session_id: [u8; limits::SESSION_ID_BYTES],
        session: Session,
    ) -> Arc<Session> {
        let arc = Arc::new(session);
        self.by_sid.insert(session_id, arc.clone());
        arc
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
    pub fn snapshot_by_user(
        &self,
        user_id: &[u8; 16],
    ) -> Vec<[u8; limits::SESSION_ID_BYTES]> {
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
