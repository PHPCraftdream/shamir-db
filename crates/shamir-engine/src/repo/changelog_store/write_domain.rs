//! Repository write-admission domain (S5c-X PR6b-2a).
//!
//! One `Arc<tokio::sync::Mutex<()>>` per repository. [`RepoWriteDomain::admit`]
//! issues an opaque [`RepoWritePermit`] that carries the repository instance's
//! unique identity token; the permit is the capability every mutation
//! implementation will require.
//!
//! Lock order: this permit is acquired FIRST — before marker persistence
//! mutex, per-table DDL admission, the raise-intent → drain-writers →
//! unique-write-lock barrier sequence, and the terminal `bindings_lock` /
//! counter `persist_lock`. The leader commit path acquires NOTHING here.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Process-global counter producing one unique token per
/// [`RepoWriteDomain`] construction. Starts at 1 (0 is never handed out).
static NEXT_REPO_WRITE_DOMAIN_TOKEN: AtomicU64 = AtomicU64::new(1);

/// The repository write-admission domain: one per repository instance.
///
/// The `repo_token` is unique per construction — NOT the name hash — so two
/// distinct instances that happen to share a name are never confused, and a
/// permit from one repository can never be presented at another.
pub struct RepoWriteDomain {
    /// Unique per construction; stamps every permit this domain issues.
    repo_token: u64,
    repo_name: String,
    mutex: tokio::sync::Mutex<()>,
    /// Outstanding-reservation hook (S5c-X PR6b-2c will use it for paused
    /// receipts): while set, ordinary [`RepoWriteDomain::admit`] is rejected;
    /// only [`RepoWriteDomain::admit_for_close`] and already-held permits
    /// proceed.
    reserved: AtomicBool,
}

/// Opaque capability issued by [`RepoWriteDomain::admit`].
///
/// Holds the domain mutex for its lifetime. `Clone` is deliberately NOT
/// derived: a permit is a unique capability, not a shareable handle.
pub struct RepoWritePermit {
    domain_token: u64,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl RepoWritePermit {
    pub(crate) fn domain_token(&self) -> u64 {
        self.domain_token
    }
}

/// Errors raised by the write-admission domain.
#[derive(Debug, thiserror::Error)]
pub enum RepoWriteError {
    #[error(
        "repository '{repo}' has an outstanding write reservation; ordinary mutations are rejected"
    )]
    Reserved { repo: String },
    #[error("repository '{repo}' cannot be admitted against itself (acquired once or rejected)")]
    SameRepository { repo: String },
}

/// RAII handle for an outstanding write reservation: while alive, ordinary
/// [`RepoWriteDomain::admit`] calls are rejected. Clears the reservation on
/// `Drop`.
// S5c-X PR6b-2b/2c: lifecycle owner / paused-receipt reservation.
#[cfg_attr(not(test), allow(dead_code))]
pub struct RepoWriteReservation {
    domain: Arc<RepoWriteDomain>,
}

impl Drop for RepoWriteReservation {
    fn drop(&mut self) {
        self.domain.reserved.store(false, Ordering::Release);
    }
}

impl RepoWriteDomain {
    /// Allocate a new domain with the next process-unique token.
    pub fn new(repo_name: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            repo_token: NEXT_REPO_WRITE_DOMAIN_TOKEN.fetch_add(1, Ordering::Relaxed),
            repo_name: repo_name.into(),
            mutex: tokio::sync::Mutex::new(()),
            reserved: AtomicBool::new(false),
        })
    }

    /// Admit an ordinary repository write.
    ///
    /// The `reserved` flag is checked AFTER acquiring the mutex (a
    /// reservation placed between a pre-lock check and the lock must win, so
    /// the single authoritative check happens inside; rejection is performed
    /// by dropping the guard and returning the error).
    pub async fn admit(self: &Arc<Self>) -> Result<RepoWritePermit, RepoWriteError> {
        let guard = self.mutex.clone().lock_owned().await;
        if self.reserved.load(Ordering::Acquire) {
            return Err(RepoWriteError::Reserved {
                repo: self.repo_name.clone(),
            });
        }
        Ok(RepoWritePermit {
            domain_token: self.repo_token,
            _guard: guard,
        })
    }

    /// Admit a repository-close write. Ignores the reservation — repository
    /// close must always be admitted.
    // S5c-X PR6b-2b/2c: lifecycle owner.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn admit_for_close(self: &Arc<Self>) -> RepoWritePermit {
        let guard = self.mutex.clone().lock_owned().await;
        RepoWritePermit {
            domain_token: self.repo_token,
            _guard: guard,
        }
    }

    /// Place the (single-slot) outstanding write reservation.
    ///
    /// Returns `Some(reservation)` on success; `None` if a reservation is
    /// already outstanding.
    // S5c-X PR6b-2b/2c: paused-receipt reservation.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn try_place_reservation(self: &Arc<Self>) -> Option<RepoWriteReservation> {
        self.reserved
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| RepoWriteReservation {
                domain: Arc::clone(self),
            })
    }

    // S5c-X PR6b-2b/2c: lifecycle owner / paused-receipt reservation.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn has_outstanding_reservation(&self) -> bool {
        self.reserved.load(Ordering::Acquire)
    }

    /// Identity check required by the design: the presented permit must have
    /// been issued by THIS exact repository instance — checked against the
    /// instance's unique token, not by name and not by `try_lock`.
    pub(crate) fn debug_check_permit(&self, permit: &RepoWritePermit) {
        debug_assert_eq!(
            permit.domain_token(),
            self.repo_token,
            "permit was issued by a different RepoWriteDomain instance (repo '{}')",
            self.repo_name
        );
    }

    /// Instance-unique token accessor (dual-admission ordering).
    pub(crate) fn token(&self) -> u64 {
        self.repo_token
    }

    pub fn repo_name(&self) -> &str {
        &self.repo_name
    }

    /// Admit a PAIR of repositories for a cross-repository mutation (e.g.
    /// an online migration touching source and destination). Both domains'
    /// mutexes are acquired in ASCENDING unique-token order, so any two
    /// callers admitting the same pair in either argument order (migration
    /// A→B vs B→A) acquire in an identical global order — no deadlock.
    /// Equal identities are REJECTED, not collapsed: the design is
    /// "acquired once or rejected" — a migration source and destination
    /// that resolve to the same repository instance is a caller bug.
    pub async fn admit_pair(
        a: &Arc<Self>,
        b: &Arc<Self>,
    ) -> Result<(RepoWritePermit, RepoWritePermit), RepoWriteError> {
        if Arc::ptr_eq(a, b) {
            return Err(RepoWriteError::SameRepository {
                repo: a.repo_name.clone(),
            });
        }
        // Ascending token order; ties are impossible (tokens are unique).
        let (first, second) = if a.token() < b.token() {
            (a, b)
        } else {
            (b, a)
        };
        let first_permit = first.admit().await?;
        let second_permit = second.admit().await?;
        // Restore the caller's (source, destination) argument order.
        if first.token() == a.token() {
            Ok((first_permit, second_permit))
        } else {
            Ok((second_permit, first_permit))
        }
    }
}
