//! Server-side resumption flow (SESSION_RESUMPTION §5).
//!
//! End-to-end ticket consumption: parse envelope → decrypt with current/previous
//! ticket_key → validate version/expires_at_ns/user/tickets_invalid_before_ns
//! (strict `>`) → anti-downgrade → atomic per-(user, family) counter CAS →
//! create new [`Session`] → emit new ticket.
//!
//! The per-(user, family) counter store is in-memory; production deployments
//! MUST persist via `SystemStore` with synchronous fsync (spec §1.3
//! IMPLEMENTATION_GUIDE NORMATIVE) — wired here as a trait so callers can
//! plug in their durable backend.

use crate::common::crypto::{aes256gcm_cipher, random_array, random_bytes, Aes256GcmCipher};
use crate::common::error::{Error, Result};
use crate::common::time::UnixNanos;
use crate::common::types::{limits, BindingMode, TransportKind};
use crate::server::rotation::ServerIdentityState;
use crate::server::session::{Session, SessionPermissions, SessionStore};
use crate::server::ticket::{
    check_anti_downgrade, decrypt_ticket_with_ciphers, encrypt_ticket, encrypt_ticket_with_cipher,
    ticket_limits, validate_ticket_enums, TicketPlain, TicketWire,
};
use dashmap::DashMap;
use fxhash::FxHasher;
use std::hash::BuildHasherDefault;
use std::sync::Arc;

/// Fast non-cryptographic hasher alias used for in-memory dashmaps.
type FxBuild = BuildHasherDefault<FxHasher>;

/// Map key: `(user_id, family_id)`.
type CounterKey = ([u8; 16], [u8; limits::TICKET_FAMILY_ID_BYTES]);
/// Map value: `(last_counter, last_observed_at_ns)`.
type CounterValue = (u64, u64);

/// Pluggable per-(user_id, family_id) counter store.
///
/// Production: persistent K-V with synchronous fsync (spec §6.2). Tests +
/// development: in-memory [`InMemoryConsumedCounters`] (below).
pub trait ConsumedCounterStore: Send + Sync {
    /// Atomic compare-and-swap: if `new_counter > stored`, set stored = new and return true.
    /// Else return false (replay or stale).
    ///
    /// Implementation **MUST** persist durably (fsync) before returning `true`.
    fn try_advance(
        &self,
        user_id: &[u8; 16],
        family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
        new_counter: u64,
    ) -> bool;

    /// Background GC: remove entries with `last_observed_at + max_chain_age < now_ns`.
    /// Default impl is no-op (caller may override).
    fn gc(&self, _now_ns: u64) {}
}

/// In-memory counter store — DashMap of `(user_id, family_id) → (last_counter, last_observed_at_ns)`.
#[derive(Debug, Default)]
pub struct InMemoryConsumedCounters {
    map: DashMap<CounterKey, CounterValue, FxBuild>,
}

impl InMemoryConsumedCounters {
    /// Empty store.
    pub fn new() -> Self {
        Self {
            map: DashMap::with_hasher(FxBuild::default()),
        }
    }

    /// Number of distinct (user, family) lineages tracked.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Test helper: peek the current counter for a (user, family).
    pub fn peek(
        &self,
        user_id: &[u8; 16],
        family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
    ) -> Option<u64> {
        self.map.get(&(*user_id, *family_id)).map(|v| v.0)
    }
}

impl ConsumedCounterStore for InMemoryConsumedCounters {
    fn try_advance(
        &self,
        user_id: &[u8; 16],
        family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
        new_counter: u64,
    ) -> bool {
        let key = (*user_id, *family_id);
        let now_ns = UnixNanos::now().as_u64();
        let mut accepted = false;
        self.map
            .entry(key)
            .and_modify(|(c, ts)| {
                if new_counter > *c {
                    *c = new_counter;
                    *ts = now_ns;
                    accepted = true;
                }
            })
            .or_insert_with(|| {
                accepted = true;
                (new_counter, now_ns)
            });
        accepted
    }

    fn gc(&self, now_ns: u64) {
        let cutoff = now_ns.saturating_sub(ticket_limits::RESUMPTION_MAX_CHAIN_AGE_NS);
        self.map.retain(|_, (_, ts)| *ts >= cutoff);
    }
}

/// Server resumption configuration.
///
/// **Optim #3:** AES-256-GCM ciphers are pre-scheduled once at construction
/// time and reused across every resume. Rebuilding the key schedule per
/// call (~10% of AES-GCM total time) is eliminated.
pub struct ResumeConfig {
    /// Current `ticket_key` for AES-GCM.
    pub ticket_key: [u8; 32],
    /// Optional previous `ticket_key` for 24h overlap.
    pub ticket_key_previous: Option<[u8; 32]>,
    /// Strict mode: refuse browser → native upgrade (spec §6.1).
    pub allow_browser_ticket_upgrade: bool,
    /// Refuse plain → TLS upgrade (spec §6.4).
    pub disable_plain_ticket_upgrade: bool,
    /// Pre-scheduled cipher for `ticket_key`. Lazily built on first use via
    /// [`ResumeConfig::ciphers`]; never rebuilt afterward.
    cached_current: std::sync::OnceLock<Aes256GcmCipher>,
    /// Pre-scheduled cipher for `ticket_key_previous` if set.
    cached_previous: std::sync::OnceLock<Option<Aes256GcmCipher>>,
}

impl ResumeConfig {
    /// Construct with default flags (anti-downgrade rules per spec §6.1/§6.4).
    pub fn new(
        ticket_key: [u8; 32],
        ticket_key_previous: Option<[u8; 32]>,
        allow_browser_ticket_upgrade: bool,
        disable_plain_ticket_upgrade: bool,
    ) -> Self {
        Self {
            ticket_key,
            ticket_key_previous,
            allow_browser_ticket_upgrade,
            disable_plain_ticket_upgrade,
            cached_current: std::sync::OnceLock::new(),
            cached_previous: std::sync::OnceLock::new(),
        }
    }

    /// Get (lazily-constructed, then cached) AES-GCM ciphers for the current
    /// and optional previous keys. Subsequent calls reuse the same instances.
    pub fn ciphers(&self) -> (&Aes256GcmCipher, Option<&Aes256GcmCipher>) {
        let current = self.cached_current.get_or_init(|| {
            aes256gcm_cipher(&self.ticket_key).expect("AES-256 key length validated")
        });
        let previous = self
            .cached_previous
            .get_or_init(|| {
                self.ticket_key_previous
                    .as_ref()
                    .map(|k| aes256gcm_cipher(k).expect("AES-256 key length validated"))
            })
            .as_ref();
        (current, previous)
    }
}

/// Per-spec §5.4 inputs from the client.
#[derive(Debug, Clone)]
pub struct ResumeRequest<'a> {
    /// Wire-encoded ticket from `auth_ok.resumption_ticket`.
    pub ticket_wire_bytes: &'a [u8],
    /// Fresh client nonce.
    pub client_nonce: [u8; 32],
    /// Binding mode of the NEW connection.
    pub binding_mode_now: BindingMode,
    /// `tls_exporter_or_zeros` of the NEW connection.
    pub channel_binding_now: [u8; 32],
}

/// Server response.
#[derive(Debug)]
pub struct ResumeOk {
    /// New session id.
    pub session_id: [u8; limits::SESSION_ID_BYTES],
    /// Absolute session expiry.
    pub expires_at_ns: u64,
    /// New resumption ticket (same family_id, counter+1) — optional.
    pub resumption_ticket: Option<Vec<u8>>,
    /// New ticket expiry.
    pub resumption_expires_at_ns: Option<u64>,
}

/// Lookup hook: returns user's `tickets_invalid_before_ns` for the spec §5.4
/// step 9 check. Caller wires this to its `__system__/users` lookup.
pub trait UserStateLookup: Send + Sync {
    /// Returns `tickets_invalid_before_ns` if the user exists, `None` if not.
    fn lookup(&self, user_id: &[u8; 16]) -> Option<u64>;
}

/// Process a resume request end-to-end. Returns `Ok(ResumeOk)` on success or
/// [`Error::AuthFailed`] (generic) on any failure (spec §5.5).
///
/// `identity_state` is consulted to enforce spec §5.7 NORMATIVE / diagram 12:
/// tickets carrying a stale `identity_key_version` (i.e., issued before the
/// current keypair) are rejected so orphan clients are forced through full
/// SCRAM and pick up the `rotation_in_progress` payload (§6.5).
#[allow(clippy::too_many_arguments)]
pub fn process_resume(
    request: &ResumeRequest,
    config: &ResumeConfig,
    counters: &dyn ConsumedCounterStore,
    user_lookup: &dyn UserStateLookup,
    session_store: &SessionStore,
    identity_state: &ServerIdentityState,
    session_max_age_ns: u64,
    new_ticket_ttl_ns: u64,
    now_ns: u64,
) -> Result<ResumeOk> {
    // Step 1: parse envelope.
    let wire = TicketWire::from_bytes(request.ticket_wire_bytes).map_err(|_| Error::AuthFailed)?;

    // Step 2: validate envelope.version (we only accept v1).
    if wire.version != 1 {
        return Err(Error::AuthFailed);
    }

    // Steps 3-4: decrypt with current key, fall back to previous.
    // Optim #3: ciphers are pre-scheduled inside ResumeConfig — no AES key
    // schedule rebuild per call.
    let (current_cipher, previous_cipher) = config.ciphers();
    let plain = decrypt_ticket_with_ciphers(current_cipher, previous_cipher, &wire)
        .map_err(|_| Error::AuthFailed)?;

    // Step 5: parse plaintext + Step 6 already enforced inside decrypt_ticket
    // via the `plain.version != wire.version` check.
    let (_transport_at_auth, binding_at_auth) =
        validate_ticket_enums(&plain).map_err(|_| Error::AuthFailed)?;

    // Step 7: expiry.
    if plain.expires_at_ns <= now_ns {
        return Err(Error::AuthFailed);
    }
    if plain.original_auth_at_ns + ticket_limits::RESUMPTION_MAX_CHAIN_AGE_NS <= now_ns {
        return Err(Error::AuthFailed);
    }

    // Step 7.5: identity-key version check (spec §5.7 / diagram 12).
    // Pre-rotation tickets are rejected so orphan clients re-auth and receive
    // the `rotation_in_progress` payload they need to update their pin.
    if !identity_state.is_ticket_version_acceptable(plain.identity_key_version) {
        return Err(Error::AuthFailed);
    }

    // Step 8: lookup user → get tickets_invalid_before_ns.
    // Optim #2: `plain.user_id` is `ByteArray<16>` — direct array deref.
    let user_id: [u8; 16] = *plain.user_id.as_ref();
    let invalid_before = user_lookup.lookup(&user_id).ok_or(Error::AuthFailed)?;

    // Step 9: STRICT > comparison (spec §5.4 step 9).
    if plain.original_auth_at_ns <= invalid_before {
        return Err(Error::AuthFailed);
    }

    // Step 10: anti-downgrade.
    check_anti_downgrade(
        binding_at_auth,
        request.binding_mode_now,
        config.allow_browser_ticket_upgrade,
    )
    .map_err(|_| Error::AuthFailed)?;
    if config.disable_plain_ticket_upgrade
        && binding_at_auth == BindingMode::None
        && request.binding_mode_now != BindingMode::None
    {
        return Err(Error::AuthFailed);
    }

    // Step 11: atomic per-(user, family) counter CAS.
    // Optim #2: `plain.ticket_family_id` is `ByteArray<16>` — no copy/parse.
    let family_id: [u8; 16] = *plain.ticket_family_id.as_ref();
    if !counters.try_advance(&user_id, &family_id, plain.family_counter) {
        return Err(Error::AuthFailed);
    }

    // Step 12: create new Session.
    let session_id = random_array::<{ limits::SESSION_ID_BYTES }>();
    let expires_at_ns = now_ns.saturating_add(session_max_age_ns);

    // Step 12 + 13 fused. Optim #6: avoid deep-cloning `plain.username_nfc`
    // / `plain.roles` twice (once for Session, once for new ticket) by
    // moving them between `plain` → `new_plain` → `Session` instead. We
    // pay at most ONE String + ONE Vec<String> clone per resume regardless
    // of whether a refresh ticket is issued (down from 2 + 2 = 4 in the
    // previous version).

    let issue_refresh = plain.original_auth_at_ns + ticket_limits::RESUMPTION_MAX_CHAIN_AGE_NS
        > now_ns + new_ticket_ttl_ns;

    let transport_at_auth = plain.transport_kind_at_auth;
    let session_transport = TransportKind::from_u8(transport_at_auth).unwrap_or(TransportKind::Tcp);

    let (resumption_ticket, resumption_expires_at_ns) = if issue_refresh {
        // Refresh path: clone what Session also needs, MOVE everything else
        // into new_plain (no clone for the heavy fields).
        let session_username = plain.username_nfc.clone();
        let session_roles = plain.roles.clone();

        let new_plain = TicketPlain {
            version: 1,
            user_id: plain.user_id,           // ByteArray<16>: Copy, free
            username_nfc: plain.username_nfc, // moved
            transport_kind_at_auth: transport_at_auth,
            binding_mode_at_auth: plain.binding_mode_at_auth,
            channel_binding_at_auth: plain.channel_binding_at_auth, // ByteArray<32>: Copy
            ticket_family_id: plain.ticket_family_id,               // ByteArray<16>: Copy
            original_auth_at_ns: plain.original_auth_at_ns,
            expires_at_ns: now_ns.saturating_add(new_ticket_ttl_ns),
            family_counter: plain.family_counter.saturating_add(1),
            roles: plain.roles, // moved
            identity_key_version: plain.identity_key_version,
        };
        // Optim #3: reuse cached current cipher for re-encrypt.
        let wire = encrypt_ticket_with_cipher(current_cipher, &new_plain)
            .map_err(|_| Error::AuthFailed)?;

        let session = Session::new(
            user_id,
            session_username,
            SessionPermissions::from_roles(session_roles),
            session_transport,
            request.binding_mode_now,
            request.channel_binding_now,
            now_ns,
        );
        session_store.insert(session_id, session);

        (Some(wire.to_bytes()), Some(new_plain.expires_at_ns))
    } else {
        // No refresh: move plain directly into Session — zero clones.
        let session = Session::new(
            user_id,
            plain.username_nfc,                          // moved
            SessionPermissions::from_roles(plain.roles), // moved
            session_transport,
            request.binding_mode_now,
            request.channel_binding_now,
            now_ns,
        );
        session_store.insert(session_id, session);
        (None, None)
    };

    Ok(ResumeOk {
        session_id,
        expires_at_ns,
        resumption_ticket,
        resumption_expires_at_ns,
    })
}

/// Build a fresh ticket for a brand-new SCRAM auth.
///
/// Used right after `auth_ok` when the server wants to issue a resumption
/// token. `family_id = random(16)`, `family_counter = 1`, `original_auth_at_ns = now`.
///
/// `roles` is the permissions snapshot from the just-completed handshake —
/// SESSION_RESUMPTION §2.1 mandates that resume rebuild [`SessionPermissions`]
/// from this list. `identity_key_version` lets later rotations reject
/// pre-rotation tickets (spec §5.7 / diagram 12).
#[allow(clippy::too_many_arguments)]
pub fn issue_initial_ticket(
    ticket_key: &[u8; 32],
    user_id: [u8; 16],
    username_nfc: String,
    transport_kind_at_auth: u8,
    binding_mode_at_auth: u8,
    channel_binding_at_auth: [u8; 32],
    roles: Vec<String>,
    identity_key_version: u64,
    now_ns: u64,
    ttl_ns: u64,
) -> Result<(Vec<u8>, u64)> {
    let mut family = [0u8; limits::TICKET_FAMILY_ID_BYTES];
    random_bytes(&mut family);

    let plain = TicketPlain {
        version: 1,
        user_id: serde_bytes::ByteArray::new(user_id),
        username_nfc,
        transport_kind_at_auth,
        binding_mode_at_auth,
        channel_binding_at_auth: serde_bytes::ByteArray::new(channel_binding_at_auth),
        ticket_family_id: serde_bytes::ByteArray::new(family),
        original_auth_at_ns: now_ns,
        expires_at_ns: now_ns.saturating_add(ttl_ns),
        family_counter: 1,
        roles,
        identity_key_version,
    };
    let wire = encrypt_ticket(ticket_key, &plain).map_err(|_| Error::AuthFailed)?;
    Ok((wire.to_bytes(), plain.expires_at_ns))
}

/// Lookup adapter: implement [`UserStateLookup`] for any closure.
impl<F> UserStateLookup for F
where
    F: Fn(&[u8; 16]) -> Option<u64> + Send + Sync,
{
    fn lookup(&self, user_id: &[u8; 16]) -> Option<u64> {
        self(user_id)
    }
}

/// In-memory user store: simple hashmap-based [`UserStateLookup`].
pub type InMemoryUserStateMap = Arc<DashMap<[u8; 16], u64, FxBuild>>;

/// Construct an empty in-memory user state map.
pub fn new_user_state_map() -> InMemoryUserStateMap {
    Arc::new(DashMap::with_hasher(FxBuild::default()))
}

impl UserStateLookup for InMemoryUserStateMap {
    fn lookup(&self, user_id: &[u8; 16]) -> Option<u64> {
        self.get(user_id).map(|v| *v)
    }
}

// `parse_user_id` and `parse_family_id` removed in Optim #2 — fields are
// now `serde_bytes::ByteArray<N>` and accessed directly as `[u8; N]`.
