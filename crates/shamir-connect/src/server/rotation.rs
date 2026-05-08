//! Server-side identity rotation state machine (spec §6.4, §6.5, §12.2).

use crate::common::crypto::Ed25519Keypair;
use crate::common::error::{Error, Result};
use crate::common::rotation::{build_rotate_event_input, build_rotation_proof_input};
use crate::common::time::ns;

/// Rotation overlap window — fixed 7 days per spec §12.2.
pub const ROTATION_OVERLAP_NS: u64 = 7 * ns::DAY;

/// Server identity state — current keypair plus optional previous
/// (during the 7-day overlap).
pub struct ServerIdentityState {
    inner: parking_lot::RwLock<ServerIdentityInner>,
}

struct ServerIdentityInner {
    current: Ed25519Keypair,
    previous: Option<Ed25519Keypair>,
    rotation_until_ns: Option<u64>,
}

impl ServerIdentityState {
    /// Construct from a freshly-generated keypair.
    pub fn fresh() -> Self {
        Self {
            inner: parking_lot::RwLock::new(ServerIdentityInner {
                current: Ed25519Keypair::generate(),
                previous: None,
                rotation_until_ns: None,
            }),
        }
    }

    /// Construct from explicit material (for rehydration from `__system__/server_meta`).
    pub fn from_material(
        current_seed: &[u8; 32],
        previous_seed: Option<&[u8; 32]>,
        rotation_until_ns: Option<u64>,
    ) -> Self {
        Self {
            inner: parking_lot::RwLock::new(ServerIdentityInner {
                current: Ed25519Keypair::from_seed(current_seed),
                previous: previous_seed.map(Ed25519Keypair::from_seed),
                rotation_until_ns,
            }),
        }
    }

    /// Current public key.
    pub fn current_pub(&self) -> [u8; 32] {
        self.inner.read().current.public_bytes()
    }

    /// Previous public key during overlap (if any).
    pub fn previous_pub(&self) -> Option<[u8; 32]> {
        self.inner.read().previous.as_ref().map(|kp| kp.public_bytes())
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
        self.inner
            .read()
            .previous
            .as_ref()
            .map(|kp| kp.sign(msg))
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
        Ok(RotationOutcome {
            old_pub,
            new_pub,
            transition_until_ns,
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

    let payload = build_rotate_event_input(&old_pub, &new_pub, transition_until_ns, recipient_session_id);
    let signed_by_old = previous.sign(&payload);

    Ok(IdentityRotationEvent {
        old_pub,
        new_pub,
        transition_until_ns,
        recipient_session_id: *recipient_session_id,
        signed_by_old,
    })
}

/// Build the orphan-recovery `rotation_proof` for embedding into `auth_ok`
/// (spec §6.5).
pub fn build_rotation_in_progress_payload(
    state: &ServerIdentityState,
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

    let proof_payload =
        build_rotation_proof_input(&previous_pub, &current_pub, transition_until_ns);
    let rotation_proof = previous.sign(&proof_payload);

    Ok(RotationInProgressPayload {
        previous_pub,
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
/// offline-client orphan recovery (spec §6.5).
#[derive(Debug, Clone)]
pub struct RotationInProgressPayload {
    /// Old Ed25519 pub.
    pub previous_pub: [u8; 32],
    /// End of overlap.
    pub transition_until_ns: u64,
    /// Ed25519 signature by previous priv over the canonical input.
    pub rotation_proof: [u8; 64],
}
