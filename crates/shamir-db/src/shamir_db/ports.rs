//! Identity seam traits — the bridge between shamir-db (identity-agnostic
//! engine) and the embedding layer's real principal directory.
//!
//! shamir-db consumes opaque `principal64` ids in `Actor`/owner/group-member
//! positions and never authenticates anyone itself. The two traits here are
//! the narrow injected surface lets the engine (a) resolve/enumerate
//! principals for introspection (`access_tree`, `List`, owner-delegation
//! scope lookup) and (b) drive user-administration writes through the real
//! durable directory instead of its own historically-ineffective Store B.
//!
//! Both are `Option<Arc<dyn ...>>` fields on [`crate::shamir_db::ShamirDb`],
//! defaulting to `None` (embedded/no-directory deployments). See
//! `docs/design/identity-privilege-unification-548-549-decision.md` §3.1 for
//! the design rationale.
//!
//! ## Dependency direction
//!
//! These traits live in `shamir-db` and are *implemented* by the embedding
//! layer (`shamir-server`, over `FjallUserDirectory`). `shamir-db` MUST NOT
//! depend on `shamir-server` (the reverse is true); the seam is what keeps
//! that invariant clean.

use async_trait::async_trait;

/// Boxed error type for the write port. Chosen over a concrete enum because
/// the implementing layer (`shamir-server`) returns its own directory error
/// type (`shamir_connect::common::error::Error`) that `shamir-db` cannot
/// name without inverting the dependency — and the brief explicitly forbids
/// inventing a new error type. Every consumer of the port stringifies the
/// error into a `BatchError::QueryError` message, so the dynamic-dispatch
/// cost is irrelevant (these are admin-frequency ops).
pub type PortError = Box<dyn std::error::Error + Send + Sync>;

/// Read-only projection of one principal, as seen by the embedding layer's
/// directory. Carries the projection key (`principal64`) alongside the
/// human-readable + scope fields because `list()` returns a `Vec` with no
/// external key to match entries against.
#[derive(Debug, Clone)]
pub struct PrincipalInfo {
    /// 63-bit projection of `user_id` — the opaque id the engine stores in
    /// `Actor::User(_)` / owner / group-member positions.
    pub principal64: u64,
    /// Username (login name).
    pub name: String,
    /// The directory's stable 128-bit id for this principal.
    pub user_id: [u8; 16],
    /// Optional database scope (owner-delegation: a database owner may
    /// manage users scoped to their own database). `None` for global users.
    pub database: Option<String>,
    /// First-class superuser flag (task #557). Distinct from any role
    /// string — the literal `"superuser"` role is reserved at the directory
    /// write boundary.
    pub superuser: bool,
}

/// Read-only principal resolution, implemented by the embedding layer over
/// its real directory. Injected as `Option<Arc<dyn PrincipalResolver>>` on
/// [`crate::shamir_db::ShamirDb`].
///
/// When absent (embedded/no-directory deployments, most tests): names
/// resolve to `None`, `access_tree`/`List` principals sections are empty,
/// and owner-delegation scope lookup degrades to "global-admin only"
/// (documented safe-but-degraded behaviour, per design doc §3.1).
pub trait PrincipalResolver: Send + Sync {
    /// Resolve a single principal by its `principal64` projection key.
    /// `None` if unknown/removed.
    fn resolve(&self, principal64: u64) -> Option<PrincipalInfo>;

    /// Enumerate every known principal. O(N) full-directory scan —
    /// acceptable, this mirrors the existing `access_tree`/`List`
    /// introspection cost model exactly (both are already O(N) over all
    /// principals today).
    fn list(&self) -> Vec<PrincipalInfo>;

    /// Resolve a principal by username. Default-implemented as a linear
    /// scan over [`Self::list`] — `list()` is already O(N) and admin ops
    /// are low-frequency, so forcing every impl to maintain a second
    /// name-keyed index would be unjustified. Override only if an impl has
    /// a cheaper direct lookup available.
    fn resolve_by_name(&self, name: &str) -> Option<PrincipalInfo> {
        self.list().into_iter().find(|p| p.name == name)
    }
}

/// Write-side user-administration port, implemented by the embedding layer
/// over its real durable directory. Injected as
/// `Option<Arc<dyn UserAdminPort>>` on [`crate::shamir_db::ShamirDb`].
///
/// When absent, the four re-targeted handlers (`handle_create_user` /
/// `handle_drop_user` / `handle_grant_role` / `handle_revoke_role`) return a
/// typed `not_supported` — the retirement of Store B is a HARD behavioural
/// cutover, not a soft fallback (design doc §3.1: "Without an installed
/// port these ops return a typed `not_supported`").
///
/// `set_superuser` exists for trait completeness/symmetry; the live wire
/// path for `SetSuperuser` stays exactly as task #557 built it (a top-level
/// `DbRequest`, NOT routed through this port — see task #559 brief §3).
#[async_trait]
pub trait UserAdminPort: Send + Sync {
    /// Create a new user. `password` is plaintext — Argon2id derivation
    /// happens INSIDE the port impl (shamir-db never touches SCRAM crypto,
    /// per design doc §3.1). Returns the new 128-bit `user_id`.
    async fn create_user(
        &self,
        name: &str,
        password: &str,
        roles: Vec<String>,
        database: Option<String>,
    ) -> Result<[u8; 16], PortError>;

    /// Drop a user by name. Returns `Ok(true)` if the account existed and
    /// was removed, `Ok(false)` if it was already absent (idempotent).
    async fn drop_user(&self, name: &str) -> Result<bool, PortError>;

    /// Grant a role string to a user. The literal `"superuser"` is reserved
    /// (rejected at the directory write boundary — use `set_superuser` for
    /// the flag).
    async fn grant_role(&self, user: &str, role: &str) -> Result<(), PortError>;

    /// Revoke a role string from a user.
    async fn revoke_role(&self, user: &str, role: &str) -> Result<(), PortError>;

    /// Grant or revoke the first-class superuser flag. Provided for trait
    /// completeness; no live shamir-db-internal caller uses it today (the
    /// wire path is the top-level `DbRequest::SetSuperuser` from #557).
    async fn set_superuser(&self, user: &str, on: bool) -> Result<(), PortError>;
}
