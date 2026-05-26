//! Server-side identity rotation state machine (spec §6.4, §6.5, §12.2).

use crate::common::crypto::Ed25519Keypair;
use crate::common::error::{Error, Result};
use crate::common::rotation::{build_rotate_event_input, build_rotation_proof_input};
use crate::common::time::ns;

/// Rotation overlap window — fixed 7 days per spec §12.2.
pub const ROTATION_OVERLAP_NS: u64 = 7 * ns::DAY;

/// Server identity state — current keypair plus optional previous
/// (during the 7-day overlap).
///
/// **Optim #5:** `current_version` is mirrored to a `std::sync::atomic::AtomicU64`
/// so `is_ticket_version_acceptable` (called per-resume) can read it
/// lock-free with `Relaxed` ordering instead of acquiring the RwLock. Saves
/// ~3 ns per resume.
pub struct ServerIdentityState {
    inner: parking_lot::RwLock<ServerIdentityInner>,
    /// Lock-free mirror of `inner.current_version`. Updated under the same
    /// write lock as `inner.current_version` in [`Self::rotate`] so they
    /// stay coherent for resume-path reads.
    current_version_atomic: std::sync::atomic::AtomicU64,
}

struct ServerIdentityInner {
    current: Ed25519Keypair,
    previous: Option<Ed25519Keypair>,
    rotation_until_ns: Option<u64>,
    /// Monotonically increasing version of the current Ed25519 keypair.
    /// Starts at 0 for `fresh()`; `rotate()` increments by 1. Allows tickets
    /// to record which keypair they were issued under so that during a
    /// rotation overlap window the server can reject pre-rotation tickets
    /// (spec §5.7 NORMATIVE / diagram 12).
    current_version: u64,
}

impl ServerIdentityState {
    /// Construct from a freshly-generated keypair (`current_version = 0`).
    pub fn fresh() -> Self {
        Self {
            inner: parking_lot::RwLock::new(ServerIdentityInner {
                current: Ed25519Keypair::generate(),
                previous: None,
                rotation_until_ns: None,
                current_version: 0,
            }),
            current_version_atomic: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Construct from explicit material (for rehydration from `__system__/server_meta`).
    ///
    /// `current_version` MUST match the persisted version counter so that
    /// post-restart resumes correctly accept/reject in-flight tickets.
    pub fn from_material(
        current_seed: &[u8; 32],
        previous_seed: Option<&[u8; 32]>,
        rotation_until_ns: Option<u64>,
        current_version: u64,
    ) -> Self {
        Self {
            inner: parking_lot::RwLock::new(ServerIdentityInner {
                current: Ed25519Keypair::from_seed(current_seed),
                previous: previous_seed.map(Ed25519Keypair::from_seed),
                rotation_until_ns,
                current_version,
            }),
            current_version_atomic: std::sync::atomic::AtomicU64::new(current_version),
        }
    }

    /// Current identity-key version.
    ///
    /// Optim #5: lock-free atomic read.
    pub fn current_version(&self) -> u64 {
        self.current_version_atomic
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether a ticket carrying `ticket_identity_key_version` is acceptable
    /// for resumption right now (spec §5.7 NORMATIVE / diagram 12).
    ///
    /// Rules:
    /// - Outside overlap (no `previous`): accept ONLY tickets at `current_version`.
    ///   (Older tickets predate at least one rotation that already finalized;
    ///   their issuing keypair is gone — force re-auth.)
    /// - Inside overlap: ticket version MUST equal `current_version`. Tickets
    ///   issued under `previous` (i.e., `current_version - 1`) are rejected
    ///   so orphan clients are forced into full re-auth and thus pick up the
    ///   `rotation_in_progress` payload (spec §6.5 / diagram 05 Part B).
    ///
    /// Optim #5: lock-free atomic read on the resume hot path.
    #[inline]
    pub fn is_ticket_version_acceptable(&self, ticket_version: u64) -> bool {
        ticket_version
            == self
                .current_version_atomic
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Current public key.
    pub fn current_pub(&self) -> [u8; 32] {
        self.inner.read().current.public_bytes()
    }

    /// Previous public key during overlap (if any).
    pub fn previous_pub(&self) -> Option<[u8; 32]> {
        self.inner
            .read()
            .previous
            .as_ref()
            .map(|kp| kp.public_bytes())
    }

    /// `transition_until_ns` if currently in overlap.
    pub fn rotation_until_ns(&self) -> Option<u64> {
        self.inner.read().rotation_until_ns
    }

    /// Whether overlap window is currently active.
    pub fn rotation_in_progress(&self, now_ns: u64) -> bool {
        match self.inner.read().rotation_until_ns {
            Some(t) => now_ns < t,
            None => false,
        }
    }

    /// Sign a message with the **current** priv key.
    pub fn sign_with_current(&self, msg: &[u8]) -> [u8; 64] {
        self.inner.read().current.sign(msg)
    }

    /// Sign a message with the **previous** priv key (if in overlap).
    pub fn sign_with_previous(&self, msg: &[u8]) -> Option<[u8; 64]> {
        self.inner.read().previous.as_ref().map(|kp| kp.sign(msg))
    }

    /// Execute `rotateServerIdentity` (spec §12.2):
    /// - Pre-condition: not already in overlap (HIGH-5 fix).
    /// - Atomic: previous = current; current = new; rotation_until = now + 7d.
    pub fn rotate(&self, now_ns: u64) -> Result<RotationOutcome> {
        let mut g = self.inner.write();
        if let Some(t) = g.rotation_until_ns {
            if now_ns < t {
                return Err(Error::InvalidInput("rotation_in_progress_already"));
            }
        }
        let old = std::mem::replace(&mut g.current, Ed25519Keypair::generate());
        let new_pub = g.current.public_bytes();
        let old_pub = old.public_bytes();
        let transition_until_ns = now_ns.saturating_add(ROTATION_OVERLAP_NS);
        g.previous = Some(old);
        g.rotation_until_ns = Some(transition_until_ns);
        g.current_version = g.current_version.saturating_add(1);
        // Optim #5: keep atomic mirror in sync — happens under the same
        // write lock so readers via `is_ticket_version_acceptable` see a
        // consistent value once they observe the new atomic load.
        self.current_version_atomic
            .store(g.current_version, std::sync::atomic::Ordering::Relaxed);
        Ok(RotationOutcome {
            old_pub,
            new_pub,
            transition_until_ns,
            new_version: g.current_version,
        })
    }

    /// Background task: when `now_ns >= rotation_until_ns`, zeroize previous
    /// and clear the overlap state. Returns `true` if cleanup happened.
    pub fn try_finalize(&self, now_ns: u64) -> bool {
        let mut g = self.inner.write();
        match g.rotation_until_ns {
            Some(t) if now_ns >= t => {
                g.previous = None;
                g.rotation_until_ns = None;
                true
            }
            _ => false,
        }
    }
}

/// Result of a successful [`ServerIdentityState::rotate`] call.
#[derive(Debug, Clone)]
pub struct RotationOutcome {
    /// Old (now-previous) Ed25519 public key.
    pub old_pub: [u8; 32],
    /// New (now-current) Ed25519 public key.
    pub new_pub: [u8; 32],
    /// Wall-clock end of overlap window.
    pub transition_until_ns: u64,
    /// New `current_version` after rotation. Used by callers issuing fresh
    /// tickets immediately so they tag the ticket with the just-incremented
    /// version (spec §5.7 / diagram 12).
    pub new_version: u64,
}

/// Build the per-recipient `identity_rotation` event signed by the previous
/// priv (spec §12.2 step 4).
///
/// MUST be called on every active session — uniqueness via
/// `recipient_session_id` is what defeats replay between recipients.
pub fn build_identity_rotation_event(
    state: &ServerIdentityState,
    recipient_session_id: &[u8; 32],
) -> Result<IdentityRotationEvent> {
    let inner = state.inner.read();
    let previous = inner
        .previous
        .as_ref()
        .ok_or(Error::InvalidInput("no rotation in progress"))?;
    let transition_until_ns = inner
        .rotation_until_ns
        .ok_or(Error::InvalidInput("no rotation in progress"))?;
    let old_pub = previous.public_bytes();
    let new_pub = inner.current.public_bytes();

    let payload = build_rotate_event_input(
        &old_pub,
        &new_pub,
        transition_until_ns,
        recipient_session_id,
    );
    let signed_by_old = previous.sign(&payload);

    Ok(IdentityRotationEvent {
        old_pub,
        new_pub,
        transition_until_ns,
        recipient_session_id: *recipient_session_id,
        signed_by_old,
    })
}

/// Build the orphan-recovery payload for embedding into `auth_ok`
/// (spec §6.5 + diagram 05 Part B step 67).
///
/// Two signatures by previous_priv are produced:
///
/// 1. `identity_sig_previous` — over the **same byte-exact** `identity_input`
///    that the current keypair signed for the standard `identity_sig` field.
///    Diagram 05 Part B step 75 ("Verify identity_sig_previous против
///    previous_pub ✓") relies on this.
/// 2. `rotation_proof` — over the `(previous_pub, current_pub,
///    transition_until_ns)` chain so the client can attest that current_pub
///    was authorized by the previously-pinned key.
///
/// `identity_input` MUST be the bytes returned by
/// [`crate::common::identity::build_identity_input`] for the current handshake
/// — i.e. computed against `current_pub`, the same `auth_message`, the same
/// `session_id`, and the same `expires_at_ns` that the corresponding
/// `identity_sig` is signed over.
pub fn build_rotation_in_progress_payload(
    state: &ServerIdentityState,
    identity_input: &[u8],
) -> Result<RotationInProgressPayload> {
    let inner = state.inner.read();
    let previous = inner
        .previous
        .as_ref()
        .ok_or(Error::InvalidInput("no rotation in progress"))?;
    let transition_until_ns = inner
        .rotation_until_ns
        .ok_or(Error::InvalidInput("no rotation in progress"))?;
    let previous_pub = previous.public_bytes();
    let current_pub = inner.current.public_bytes();

    let identity_sig_previous = previous.sign(identity_input);

    let proof_payload =
        build_rotation_proof_input(&previous_pub, &current_pub, transition_until_ns);
    let rotation_proof = previous.sign(&proof_payload);

    Ok(RotationInProgressPayload {
        previous_pub,
        identity_sig_previous,
        transition_until_ns,
        rotation_proof,
    })
}

/// Wire view: `identity_rotation` event sent over the active session bus.
#[derive(Debug, Clone)]
pub struct IdentityRotationEvent {
    /// Old Ed25519 public key.
    pub old_pub: [u8; 32],
    /// New Ed25519 public key.
    pub new_pub: [u8; 32],
    /// End of overlap.
    pub transition_until_ns: u64,
    /// Full session id of the recipient (spec HIGH-5 fix — full 32 bytes).
    pub recipient_session_id: [u8; 32],
    /// Ed25519 signature by previous priv over the canonical input.
    pub signed_by_old: [u8; 64],
}

/// Wire view: `rotation_in_progress` payload embedded in `auth_ok` for
/// offline-client orphan recovery (spec §6.5 + diagram 05 Part B step 70).
///
/// Spec §6.5 NORMATIVE 4-field schema:
/// ```text
/// rotation_in_progress: {
///     previous_pub,
///     identity_sig_previous,  // Ed25519 sig by previous_priv over SAME identity_input
///     transition_until_ns,
///     rotation_proof          // Ed25519 sig by previous_priv over rotation chain
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RotationInProgressPayload {
    /// Old Ed25519 pub.
    pub previous_pub: [u8; 32],
    /// Ed25519 signature by previous_priv over the **same byte-exact**
    /// `identity_input` that current_priv signed for `identity_sig`. Allows
    /// orphan client to verify mutual auth against its old pin (spec §6.5
    /// step 3 / diagram 05 Part B step 75).
    pub identity_sig_previous: [u8; 64],
    /// End of overlap.
    pub transition_until_ns: u64,
    /// Ed25519 signature by previous priv over the
    /// `(previous_pub, current_pub, transition_until_ns)` rotation chain.
    pub rotation_proof: [u8; 64],
}
