use std::fmt;
use std::hash::{Hash, Hasher};

/// Reserved u64 id for the `System` actor in persisted owner fields.
///
/// `Actor::System` serialises to `OWNER_SYSTEM` in catalogue records;
/// `Actor::User(id)` serialises to the user's numeric id.
pub const OWNER_SYSTEM: u64 = 0;

/// Fixed 63-bit projection of a directory-minted 16-byte user id into the
/// catalogue's `i64`-safe integer space (owner / group-member encoding).
///
/// Pure truncation — NOT a hash of anything attacker-chosen. Uniqueness and
/// non-zero-ness are enforced once, at mint time, by the directory that
/// produces the 16 bytes (`FjallUserDirectory`, task #556) — this function
/// only projects. See `docs/dev-artifacts/design/identity-privilege-unification-548-549-decision.md`
/// §2.2 for the full rationale (why 63-bit projection over widening the
/// catalogue to 128 bits).
pub fn principal64(user_id: [u8; 16]) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&user_id[0..8]);
    u64::from_be_bytes(buf) & (i64::MAX as u64)
}

/// Interim, username-keyed bridge into the `principal64` id space.
///
/// **NOT the real identity primitive** — it is a deterministic hash of a
/// *name*, exactly what `principal64`'s real 16-byte projection is designed
/// to replace. It exists ONLY for the legacy call sites and test/bench
/// fixtures that cannot reach a directory-minted id. Do not add new
/// production call sites; use `principal64(session.user_id)` or the
/// `PrincipalResolver` when available.
pub fn principal64_from_username(username: &str) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    username.hash(&mut hasher);
    let hash = hasher.finish();
    let mut user_id = [0u8; 16];
    user_id[0..8].copy_from_slice(&hash.to_be_bytes());
    principal64(user_id)
}

/// The identity performing an operation.
///
/// `System` is the all-bypassing default used while the authentication
/// wire path is not yet plumbed through.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Actor {
    #[default]
    System,
    /// A superuser session. Bypasses `permits()` exactly like `System`, but
    /// carries the real principal64 id so admin-created resources retain
    /// creator ownership. Never produced by `from_owner_id`: admin status is
    /// a live session property, not a persisted owner property.
    Admin(u64),
    User(u64),
}

impl Actor {
    /// Persist-friendly u64 encoding: `System` → [`OWNER_SYSTEM`],
    /// `Admin(id)`/`User(id)` → `id`.
    pub fn to_owner_id(&self) -> u64 {
        match self {
            Self::System => OWNER_SYSTEM,
            Self::Admin(id) | Self::User(id) => *id,
        }
    }

    /// Decode a persisted owner id back into an [`Actor`].
    pub fn from_owner_id(id: u64) -> Self {
        if id == OWNER_SYSTEM {
            Self::System
        } else {
            Self::User(id)
        }
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System => f.write_str("System"),
            Self::Admin(id) => write!(f, "Admin({id})"),
            Self::User(id) => write!(f, "User({id})"),
        }
    }
}
