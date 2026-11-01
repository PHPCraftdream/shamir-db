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
use crate::server::auth::token::verify::Verified;
use crate::server::state::admission::{Admission, AdmitReject, Sid, ACTIVE_WINDOW_NS};
use crate::server::state::origin::SessionOrigin;
use arc_swap::ArcSwapOption;
use dashmap::DashMap;
use smallvec::SmallVec;
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
    /// Whether this session has replication-API access (task #621, mirrors
    /// `is_superuser` — driven by the directory's authoritative `replicator`
    /// flag, not a role string).
    pub is_replicator: bool,
    /// Roles snapshot.
    pub roles: Vec<String>,
}

impl SessionPermissions {
    /// Construct from a list of roles. `is_superuser` is true iff "superuser"
    /// is among them; `is_replicator` is true iff "replicator" is among them.
    ///
    /// **Task #557/#621 note:** the literal `"superuser"`/`"replicator"`
    /// strings are now reserved at the directory write boundary
    /// (`FjallUserDirectory::update_roles` rejects both), so any roles list
    /// produced by the real directory will never contain them. Production
    /// code that has the directory's authoritative flags should use
    /// [`Self::new`] instead. `from_roles` is retained for callers that
    /// build a `SessionPermissions` from a plain role list without touching
    /// `FjallUserDirectory` (in-memory/test directories, fixtures, resume
    /// from a legacy ticket).
    pub fn from_roles(roles: Vec<String>) -> Self {
        let is_superuser = roles.iter().any(|r| r == "superuser");
        let is_replicator = roles.iter().any(|r| r == "replicator");
        Self {
            is_superuser,
            is_replicator,
            roles,
        }
    }

    /// Construct from the directory's authoritative `superuser`/`replicator`
    /// flags plus the (now-reserved-string-free) role list. Use this instead
    /// of [`Self::from_roles`] wherever the caller has real flag values
    /// (every production call site after task #557/#621) — `from_roles`'s
    /// string-scan is kept only for callers that never had the flags.
    pub fn new(is_superuser: bool, is_replicator: bool, roles: Vec<String>) -> Self {
        Self {
            is_superuser,
            is_replicator,
            roles,
        }
    }

    /// Returns true if this session holds the named role.
    ///
    /// Used by privileged gates that key off a specific role rather than the
    /// blanket `is_superuser` flag — e.g. `require_role(session,
    /// "replicator")` for the replication pull-API (REPLICATION §5.4, PR2).
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
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
            .field("permissions", &"<immutable snapshot>")
            .field("created_at_ns", &self.created_at_ns)
            .field("transport_kind", &self.transport_kind)
            .field("binding_mode", &self.binding_mode)
            .field("channel_binding_at_auth", &"<REDACTED:32>")
            .field("origin", &self.origin)
            .field("pending_changepw_challenge", &"<ArcSwapOption>")
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
    /// `changePassword` for `auth_message_cp`.
    ///
    /// NOT compared against the resuming connection's current exporter in
    /// `process_resume` (an earlier attempt at this — audit finding 1d,
    /// 2026-07-06-security-network-surface.md — was reverted: TLS exporter
    /// values are unique per-connection even across session resumption per
    /// RFC 9266, so raw equality between this value and a NEW connection's
    /// exporter breaks every legitimate ticket-based reconnect, not just
    /// stolen-ticket replay; see `resume.rs`'s `process_resume` for the full
    /// reasoning and the follow-up task tracking a correct fix).
    pub channel_binding_at_auth: [u8; 32],
    /// Authentication origin; token origins are server-only and never enter the wire protocol.
    pub origin: SessionOrigin,
    /// In-flight changePassword challenge state.
    ///
    /// Lock-free: `ArcSwapOption` over `Arc<PendingChangePwChallenge>`.
    /// Issue replaces any prior challenge (`store(Some(...))`); consume is a
    /// single atomic `swap(None)` so exactly one caller observes a non-empty
    /// slot — the §12.5 double-submit guard with no TOCTOU window.
    pub pending_changepw_challenge: ArcSwapOption<PendingChangePwChallenge>,
    /// Post-auth request-rate token bucket (task #608, migrated off
    /// `std::sync::Mutex` per #1090). Two independent atomics, not one
    /// struct behind a lock — see [`PostAuthBucket`]'s doc for why this is
    /// sound: the load-bearing security property (no concurrent caller can
    /// double-spend a token) lives entirely in `micro_tokens`'s CAS retry
    /// loop; `last_refill_at_ns` tolerates benign raciness.
    post_auth_bucket: PostAuthBucket,
}

/// Fixed-point token-bucket state — mirrors `rate_limit.rs`'s `BucketState`
/// shape (`micro_tokens` = tokens × 1e9, refill without floats).
///
/// **Lock-free design (#1090).** `micro_tokens` and `last_refill_at_ns` are
/// two INDEPENDENT atomics rather than one struct behind a `Mutex` — this is
/// sound only because the two fields have different consistency
/// requirements:
/// - `micro_tokens` is updated via [`AtomicU64::fetch_update`]'s CAS retry
///   loop, which is a genuine atomic read-modify-write: exactly one
///   concurrent caller can transition it from `X` to `X - cost` at a time.
///   This is the ONLY property that actually matters for the rate limiter's
///   security guarantee (no concurrent request can debit a token that
///   another concurrent request already debited).
/// - `last_refill_at_ns` is updated via [`AtomicU64::fetch_max`], NOT a plain
///   `swap` (an earlier version of this fix used `swap` and asserted a false
///   "can only under-refill, never over-refill" invariant here — caught by
///   @oh review, 2026-08-11: a plain `swap` lets the watermark REGRESS under
///   concurrent callers whose `now_ns` arrive out of order, e.g. a caller
///   preempted between reading the wall clock and reaching the atomic op;
///   the next call then re-credits an already-credited interval, an
///   unbounded over-refill with no aggregate cap — `capacity()`'s per-call
///   clamp does not prevent repeated re-crediting of the SAME wall-clock
///   span across multiple calls). `fetch_max` returns the prior value
///   (same `elapsed` computation a `swap` would give) while ALSO
///   guaranteeing the stored watermark never regresses, so `elapsed` is
///   always computed against a genuinely later-or-equal point — the total
///   refill across any sequence of calls is bounded by the true wall-clock
///   span between them. This is a genuine correctness improvement over the
///   ORIGINAL `std::sync::Mutex`-guarded implementation too: that version
///   captured `now_ns` outside its own critical section and stored it
///   unconditionally, carrying the identical regression hazard.
struct PostAuthBucket {
    micro_tokens: AtomicU64,
    last_refill_at_ns: AtomicU64,
}

impl PostAuthBucket {
    /// Capacity in scaled units (= burst limit × 1e9). We allow burst of
    /// 1 second's worth of tokens, mirroring `BucketState::capacity_at_rate`
    /// in `rate_limit.rs`.
    fn capacity() -> u64 {
        (shamir_tunables::instance_defaults::POST_AUTH_RATE_LIMIT_PER_SEC as u64) * 1_000_000_000
    }
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
            user_id,
            username,
            permissions,
            created_at_ns: now_ns,
            last_activity_ns: AtomicU64::new(now_ns),
            transport_kind,
            binding_mode,
            channel_binding_at_auth,
            origin: SessionOrigin::Scram,
            pending_changepw_challenge: ArcSwapOption::const_empty(),
            post_auth_bucket: PostAuthBucket {
                micro_tokens: AtomicU64::new(PostAuthBucket::capacity()),
                last_refill_at_ns: AtomicU64::new(now_ns),
            },
        }
    }

    /// Construct a token session using the verified owner's role snapshot. Fixed SCRAM-only fields are filler and never exposed.
    pub fn for_token(
        verified: &Verified,
        username: String,
        now_ns: u64,
        session_id: [u8; 32],
    ) -> Self {
        let record = verified.state.record();
        let owner = record.owner_user_id;
        let mut session = Self::new(
            owner,
            username,
            SessionPermissions::new(false, false, record.roles.clone()),
            TransportKind::WebSocket,
            BindingMode::None,
            [0; 32],
            now_ns,
        );
        session.session_id = session_id;
        session.origin = SessionOrigin::Token(crate::server::state::origin::TokenOrigin {
            state: Arc::clone(&verified.state),
        });
        session
    }

    /// Derive the per-session HMAC key from `session_id`. Delegates to
    /// the shared [`crate::common::crypto::derive_session_hmac_key`] so
    /// server and client compute byte-identical keys from one definition
    /// (task #604).
    fn derive_hmac_key(session_id: &[u8; limits::SESSION_ID_BYTES]) -> [u8; 32] {
        crate::common::crypto::derive_session_hmac_key(session_id)
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
        Self::derive_hmac_key(&self.session_id)
    }

    /// Update `last_activity_ns` to current wall-clock.
    ///
    /// Calls `UnixNanos::now()` internally — on Windows this is a syscall
    /// (~100 ns). Hot-path callers processing many requests per batch should
    /// use [`Session::touch_at`] instead and reuse a single timestamp
    /// captured by the transport layer.
    ///
    /// Legacy wrapper; prefer [`Session::touch_at`].
    ///
    /// # Panics
    /// Panics when the system clock is invalid.
    pub fn touch(&self) {
        self.touch_at(UnixNanos::now().as_u64());
    }

    /// **Optim #5:** update `last_activity_ns` with a caller-supplied timestamp.
    ///
    /// Allows the transport layer to capture `UnixNanos::now()` once per
    /// request (or per batch) and reuse it across multiple session touches,
    /// amortizing the syscall cost. On Windows this saves ~100 ns/dispatch.
    ///
    /// Monotonic by construction: `fetch_max` (NOT a plain `store`, the same
    /// reasoning as `PostAuthBucket::last_refill_at_ns`) so a delayed or
    /// out-of-order timestamp can never REGRESS activity and make a fresh
    /// session look idle.
    #[inline]
    pub fn touch_at(&self, now_ns: u64) {
        // fetch_max, not store: activity never goes backwards (see doc).
        self.last_activity_ns.fetch_max(now_ns, Ordering::Relaxed);
    }

    /// Whether this session has expired by wall-clock.
    pub fn is_expired(&self, now_ns: u64, max_age_ns: u64, idle_ttl_ns: u64) -> bool {
        let last = self.last_activity_ns.load(Ordering::Relaxed);
        now_ns > self.created_at_ns + max_age_ns || now_ns > last + idle_ttl_ns
    }

    /// Per-request session validity check per spec §7.5 [NORMATIVE].
    ///
    /// Returns `false` if `created_at_ns <= tickets_invalid_before_ns` —
    /// caller MUST kick the session immediately and emit `session_invalidated`.
    #[inline]
    pub fn is_valid_for_user(&self, tickets_invalid_before_ns: u64) -> bool {
        self.created_at_ns > tickets_invalid_before_ns
    }

    /// Check + consume one post-auth request token (task #608). Returns
    /// `None` if allowed (bucket debited), `Some(retry_after_secs)` if
    /// rejected. Mirrors `rate_limit.rs::InMemoryRateLimiter::check`'s exact
    /// refill math (fixed-point micro_tokens, no floats), scoped to this one
    /// session instead of a per-subnet map — no warmup window (that's a
    /// pre-auth/post-restart-abuse concept, not applicable to an
    /// already-authenticated session).
    pub fn check_post_auth_rate_limit(&self, now_ns: u64) -> Option<u32> {
        let capacity = PostAuthBucket::capacity();
        let rate = shamir_tunables::instance_defaults::POST_AUTH_RATE_LIMIT_PER_SEC as u64;
        let cost = 1_000_000_000u64;

        // `fetch_max`, NOT `swap` (found by @oh review, 2026-08-11): a plain
        // `swap` lets `last_refill_at_ns` regress under concurrent callers
        // whose `now_ns` values arrive out of order (e.g. a caller preempted
        // between reading the wall clock and reaching this line) -- the
        // NEXT call then re-credits an interval that was already credited,
        // an unbounded over-refill with no aggregate cap (only a per-call
        // `min(capacity)` clamp, which doesn't prevent repeated re-crediting
        // of the SAME wall-clock interval). `fetch_max` returns the prior
        // value (same `elapsed` computation as `swap` did) while ALSO
        // guaranteeing the stored value never regresses -- every `elapsed`
        // computed from it is against a genuinely later-or-equal watermark,
        // so the total refill across any sequence of calls is bounded by
        // the true wall-clock span, closing the over-credit hole. See
        // PostAuthBucket's struct doc for the full property; the load-
        // bearing DEBIT atomicity is still the `fetch_update` CAS loop
        // below, unchanged.
        let prev_refill_ns = self
            .post_auth_bucket
            .last_refill_at_ns
            .fetch_max(now_ns, Ordering::Relaxed);
        let elapsed = now_ns.saturating_sub(prev_refill_ns);
        let refill = elapsed.saturating_mul(rate);

        // Atomic refill + debit: exactly one concurrent caller can observe
        // (and act on) tokens >= cost for a given pre-refill value, since
        // fetch_update is a genuine CAS retry loop. `admitted`/
        // `tokens_after_refill` are overwritten on every retry, so after
        // the loop they reflect the ONE closure invocation that actually
        // committed.
        let mut admitted = false;
        let mut tokens_after_refill = 0u64;
        self.post_auth_bucket
            .micro_tokens
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |tokens| {
                let refilled = tokens.saturating_add(refill).min(capacity);
                tokens_after_refill = refilled;
                if refilled >= cost {
                    admitted = true;
                    Some(refilled - cost)
                } else {
                    admitted = false;
                    Some(refilled)
                }
            })
            .expect("closure always returns Some, fetch_update never errors");

        if admitted {
            None
        } else {
            let deficit = cost.saturating_sub(tokens_after_refill);
            let secs_to_wait = (deficit / rate) / 1_000_000_000;
            Some((secs_to_wait as u32).max(1))
        }
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
type SessionIdHasher = std::hash::BuildHasherDefault<rustc_hash::FxHasher>;

/// Per-user membership slot list: up to the spec §7.4 cap of 16
/// (sid, session) pairs, stack-allocated via `SmallVec`.
type UserSlot = SmallVec<[(Sid, Arc<Session>); 16]>;
/// Contention-model map: one shard guard per user hash (see `SessionStore`).
type UserSlotMap = DashMap<[u8; 16], UserSlot, SessionIdHasher>;

#[derive(Debug)]
/// Active sessions indexed by session ID (routing) plus a per-user
/// membership index (cap accounting).
pub struct SessionStore {
    /// Routing index: bearer sid -> session. Contention model: per-shard
    /// DashMap locks keyed by sid hash; guards are short-lived, never held
    /// across `.await`, and never two at once.
    by_sid: DashMap<Sid, Arc<Session>, SessionIdHasher>,
    /// Per-user membership: user -> [(sid, session)]. Contention model: one
    /// shard guard per user hash — ALL cap decisions and slot mutation for a
    /// user happen under this guard with O(cap) work (never an O(all-sessions)
    /// scan; replaces the old global cap mutex, RV-CONNECT-D8C2825A).
    /// Lock order: `admit` takes the `by_user` guard FIRST, then a single
    /// `by_sid` guard under it — never the reverse (see `admit`'s doc).
    by_user: UserSlotMap,
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
            by_user: DashMap::with_hasher(SessionIdHasher::default()),
        }
    }

    /// Test-fixture insertion ONLY: [`SessionStore::admit`] with the cap
    /// disabled (`cap = 0`). Production code paths (login AND resume) must
    /// use [`SessionStore::admit`] so the per-user cap is always enforced.
    pub fn insert(&self, session_id: Sid, session: Session) -> Arc<Session> {
        match self.admit(session_id, session, 0, 0) {
            Admission::Admitted { session, .. } => session,
            // Unreachable: cap 0 disables the cap and fixture sids are unique.
            Admission::Rejected(_) => unreachable!("admit with cap 0 never rejects"),
        }
    }

    /// The ONLY way a session enters the store (spec §7.4 overflow policy;
    /// one function for login AND resume — RV-CONNECT-8AD8B1C8 /
    /// RV-CONNECT-0A42F42C).
    ///
    /// Under ONE per-user shard guard: (1) idempotent pruning, (2) decide
    /// with no side effects, (3) claim the sid in `by_sid`, (4) evict idle
    /// LRU victims, (5) update the slot. All work is O(cap), synchronous,
    /// never across `.await`. Lock order: the `by_user` guard may be held
    /// while taking a single `by_sid` guard — never the reverse, never two
    /// `by_sid` guards at once.
    ///
    /// Invariants: I2 exact index when nothing is in flight; I3 an admit
    /// never pushes the user above `max(cap, previous length)`; I4 a
    /// `Rejected` result changes nothing apart from idempotent pruning; I5
    /// active iff `now - last < ACTIVE_WINDOW_NS` (saturating), only idle
    /// sessions evicted in (last_activity, sid) order.
    pub fn admit(
        &self,
        session_id: Sid,
        mut session: Session,
        now_ns: u64,
        cap: usize,
    ) -> Admission {
        session.session_id = session_id;
        let user_id = session.user_id;
        let arc = Arc::new(session);

        // by_user guard: all cap accounting for this user happens under it.
        let mut slot = self.by_user.entry(user_id).or_default();

        // 1. Idempotent pruning: drop slot entries whose by_sid binding was
        //    removed or replaced (ptr_eq). One by_sid guard at a time.
        slot.retain(|(k, v)| {
            self.by_sid
                .get(k)
                .is_some_and(|b| Arc::ptr_eq(b.value(), v))
        });

        // 2. Decide with NO side effects. A replacement (same sid already in
        //    this user's slot) consumes no extra capacity. cap 0 disables
        //    the cap entirely.
        let replacing = slot.iter().any(|(k, _)| *k == session_id);
        let need = if cap == 0 {
            0
        } else if replacing {
            slot.len().saturating_sub(cap)
        } else {
            (slot.len() + 1).saturating_sub(cap)
        };
        // Victims: the first `need` entries by (last_activity, sid),
        // excluding the sid being replaced.
        let mut order: Vec<(u64, Sid, Arc<Session>)> = slot
            .iter()
            .filter(|(k, _)| *k != session_id)
            .map(|(k, v)| {
                (
                    v.last_activity_ns.load(Ordering::Relaxed),
                    *k,
                    Arc::clone(v),
                )
            })
            .collect();
        order.sort_by_key(|e| (e.0, e.1));
        let victims: SmallVec<[(Sid, Arc<Session>); 1]> = order
            .into_iter()
            .take(need)
            .map(|(_, k, v)| (k, v))
            .collect();
        // I5 classification (no side effects): active iff now - last <
        // ACTIVE_WINDOW_NS (saturating). Any active victim => reject the whole
        // admission before anything is mutated.
        if victims.iter().any(|(_, v)| {
            now_ns.saturating_sub(v.last_activity_ns.load(Ordering::Relaxed)) < ACTIVE_WINDOW_NS
        }) {
            return Admission::Rejected(AdmitReject::AllActive);
        }

        // 3. Claim the sid atomically in by_sid (a foreign user's sid is never
        //    stolen). `prev` lets a later rejection restore the old binding.
        let prev = match self.by_sid.entry(session_id) {
            dashmap::mapref::entry::Entry::Occupied(mut occ) => {
                if occ.get().user_id != user_id {
                    return Admission::Rejected(AdmitReject::SidConflict);
                }
                // Same user's replacement: swap the Arc in place.
                Some(occ.insert(Arc::clone(&arc)))
            }
            dashmap::mapref::entry::Entry::Vacant(vac) => {
                vac.insert(Arc::clone(&arc));
                None
            }
        };

        // 4. Evict the victims. The idle check is repeated INSIDE `remove_if`
        //    (shard write lock) and `lookup` touches under the shard read
        //    lock, so a concurrent touch is either seen here (victim stays,
        //    admission rolled back) or finds the session already gone.
        //    `ptr_eq` guarantees a replacement generation is never removed.
        //    With a single victim (the only case outside a cap shrink) a
        //    rollback mutates nothing; with several, victims removed before a
        //    racing touch was observed were idle (>= 60 s) and stay removed.
        let mut evicted: SmallVec<[Sid; 1]> = SmallVec::new();
        for (vk, va) in &victims {
            let removed = self.by_sid.remove_if(vk, |_, v| {
                Arc::ptr_eq(v, va)
                    && now_ns.saturating_sub(v.last_activity_ns.load(Ordering::Relaxed))
                        >= ACTIVE_WINDOW_NS
            });
            if removed.is_some() {
                slot.retain(|(k, _)| k != vk);
                evicted.push(*vk);
            } else if self
                .by_sid
                .get(vk)
                .is_some_and(|b| Arc::ptr_eq(b.value(), va))
            {
                // Still bound => it turned active: undo the claim and reject.
                match prev {
                    Some(old) => {
                        self.by_sid.insert(session_id, old);
                    }
                    None => {
                        self.by_sid
                            .remove_if(&session_id, |_, v| Arc::ptr_eq(v, &arc));
                    }
                }
                return Admission::Rejected(AdmitReject::AllActive);
            } else {
                // Removed concurrently: its capacity is already free.
                slot.retain(|(k, _)| k != vk);
            }
        }

        // 5. Update the per-user slot (victims already dropped): replace or push.
        if replacing {
            if let Some(e) = slot.iter_mut().find(|(k, _)| *k == session_id) {
                e.1 = Arc::clone(&arc);
            }
        } else {
            slot.push((session_id, Arc::clone(&arc)));
        }

        Admission::Admitted {
            session: arc,
            evicted,
            replaced: replacing,
        }
    }

    /// Look up a session by id, touching `last_activity_ns` if found.
    ///
    /// Calls `UnixNanos::now()` internally LAZILY (only on hit). On miss
    /// returns immediately without consulting the clock. Hot-path callers
    /// should still prefer [`SessionStore::lookup_at`] to share one
    /// timestamp across multiple session touches.
    ///
    /// Legacy wrapper; prefer [`SessionStore::lookup_at`].
    ///
    /// # Panics
    /// Panics when the system clock is invalid (on hit only).
    pub fn lookup(&self, sid: &[u8; limits::SESSION_ID_BYTES]) -> Option<Arc<Session>> {
        // Touch under the shard guard: admission's idle recheck (remove_if)
        // takes the shard write lock, so it observes this touch.
        self.by_sid.get(sid).map(|r| {
            r.touch();
            r.clone()
        })
    }

    /// **Optim #5:** lookup with a caller-supplied `now_ns`, avoiding a
    /// `UnixNanos::now()` call inside the hot path.
    pub fn lookup_at(
        &self,
        sid: &[u8; limits::SESSION_ID_BYTES],
        now_ns: u64,
    ) -> Option<Arc<Session>> {
        self.by_sid.get(sid).map(|r| {
            r.touch_at(now_ns);
            r.clone()
        })
    }

    /// Remove a session by id. Returns the previous session if any.
    ///
    /// Takes the `by_sid` guard FIRST and drops it, then updates the
    /// per-user slot with a ptr_eq check — a stale removal racing with an
    /// admit that replaced the sid cannot drop the replacement generation.
    pub fn remove(&self, sid: &Sid) -> Option<Arc<Session>> {
        let (_, v) = self.by_sid.remove(sid)?;
        let user = v.user_id;
        self.remove_from_user_slot(user, sid, &v);
        Some(v)
    }

    /// Drop the per-user slot entry for ONE removed generation (ptr_eq),
    /// detaching the user cell if it empties. Shared by [`SessionStore::remove`]
    /// and [`SessionStore::gc_expired`] so a stale cleanup can never drop a
    /// freshly-admitted replacement generation bound to the same sid.
    fn remove_from_user_slot(&self, user: [u8; 16], sid: &Sid, gen: &Arc<Session>) {
        if let Some(mut slot) = self.by_user.get_mut(&user) {
            slot.retain(|(k, s)| !(k == sid && Arc::ptr_eq(s, gen)));
            if slot.is_empty() {
                drop(slot);
                // Detach the empty user cell only if still empty.
                self.by_user.remove_if(&user, |_, s| s.is_empty());
            }
        }
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
    /// Returns the number of evicted sessions (only generations ACTUALLY
    /// removed are counted).
    ///
    /// Generation-safe (stale-cleanup): each victim is captured as a
    /// `(sid, Arc<Session>)` PAIR and removal re-checks BOTH `is_expired`
    /// AND `Arc::ptr_eq` via `remove_if` — a GC pass that captured an
    /// expired generation while a replacement was being admitted can never
    /// later delete the freshly-admitted REPLACEMENT with the same sid. The
    /// per-user slot is updated through the same ptr_eq helper as
    /// [`SessionStore::remove`], keeping the index exact (I2).
    pub fn gc_expired(&self, now_ns: u64, max_age_ns: u64, idle_ttl_ns: u64) -> usize {
        let victims: Vec<(Sid, Arc<Session>)> = self
            .by_sid
            .iter()
            .filter(|e| e.value().is_expired(now_ns, max_age_ns, idle_ttl_ns))
            .map(|e| (*e.key(), Arc::clone(e.value())))
            .collect();
        let mut removed = 0;
        for (sid, gen) in victims {
            let gone = self
                .by_sid
                .remove_if(&sid, |_, v| {
                    Arc::ptr_eq(v, &gen) && v.is_expired(now_ns, max_age_ns, idle_ttl_ns)
                })
                .is_some();
            if gone {
                removed += 1;
                let user = gen.user_id;
                self.remove_from_user_slot(user, &sid, &gen);
            }
        }
        removed
    }

    /// Snapshot all sessions belonging to a given `user_id` (for admin
    /// `kickSession` / `updateUser` snapshot+kill semantics, spec §12.4 / §12.6).
    /// Reads the per-user index — O(cap), never an O(all-sessions) scan.
    pub fn snapshot_by_user(&self, user_id: &[u8; 16]) -> Vec<Sid> {
        self.by_user
            .get(user_id)
            .map(|slot| slot.iter().map(|(k, _)| *k).collect())
            .unwrap_or_default()
    }

    /// Number of sessions currently held for a user. Reads the per-user
    /// index — O(cap).
    pub fn count_for_user(&self, user_id: &[u8; 16]) -> usize {
        self.by_user.get(user_id).map_or(0, |slot| slot.len())
    }

    /// Iterate over all (sid, session) pairs — for monitoring/`listSessions`.
    /// Clones nothing; the callback runs without any store guard held by the
    /// iterator beyond the per-shard read guard of `by_sid` itself.
    pub fn for_each(&self, mut f: impl FnMut(&Sid, &Session)) {
        for entry in self.by_sid.iter() {
            f(entry.key(), entry.value());
        }
    }
}
