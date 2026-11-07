//! Fixed-size striped revision state (RV-STORAGE-98f6650d).
//!
//! Replaces the previous per-key `DashMap<RecordKey, Arc<TokioMutex<()>>>`
//! publication-guard registry. Each stripe is a single `AtomicU64` packing
//! `{revision (high 32 bits), active-writer count (low 32 bits)}`:
//!
//! * `begin`  — CAS to `{revision+1, active+1}` (the revision bump is what
//!   invalidates every cache entry tagged with a pre-writer stamp).
//! * `end`    — CAS to `{revision+1, active-1}` (RAII: the [`StripeGuard`]'s
//!   `Drop` runs on panic/cancellation too). The revision bumps on BOTH
//!   edges: a fill whose stamp was captured *during* a writer (active > 0,
//!   post-begin revision) must be rejected after the writer finishes, and
//!   leaving the revision unchanged at `end` would accept exactly that
//!   entry once `active` returns to 0.
//!
//! A tagged cache entry `{rev, Slot}` may be served only when, observed
//! atomically per stripe: the stripe stamp did not change between the
//! reader's pre-lookup and post-lookup captures (`s0 == s1`), the active
//! count is `0` (no writer is in flight — multiple simultaneous writers
//! must never validate an in-flight entry, and there is deliberately NO
//! odd/even parity scheme), the entry's tag equals the current revision,
//! and the stripe is not exhausted.
//!
//! Revision/counter exhaustion is fail-closed with NO wrapping arithmetic:
//! when the revision field reaches its maximum it saturates there and the
//! stripe is marked exhausted **permanently** — every future validation
//! for that stripe returns `false` (caching is disabled for that stripe).
//! Likewise, if the active-writer count field is at its maximum `begin`
//! latches exhaustion and returns an *uncounted* guard (tracked as a
//! `bool` on the guard) instead of overflowing: the guard's `end` then
//! performs only the revision bump, never a count decrement.
//!
//! Readers never block: they only load stamps. Writers never block each
//! other across different stripes, and same-stripe writers only contend on
//! a single CAS word.

use crate::types::RecordKey;
use shamir_collections::THasher;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Number of stripes. Fixed metadata — chosen once at construction; the
/// workspace `THasher` routes a key to its stripe.
pub(super) const STRIPES: usize = 256;

/// Width of the active-writer count field (low bits of the packed word).
const ACTIVE_BITS: u32 = 32;
const ACTIVE_MASK: u64 = (1u64 << ACTIVE_BITS) - 1;
/// Maximum revision value — reaching it exhausts the stripe (fail-closed).
const REV_MAX: u64 = ACTIVE_MASK;

#[derive(Debug, Default)]
pub(super) struct StripeState {
    /// Packed `{revision << ACTIVE_BITS | active_count}`.
    packed: AtomicU64,
    /// Fail-closed exhaustion latch: once `true`, every validation for this
    /// stripe returns `false` forever (caching permanently disabled for the
    /// stripe). Never reset.
    exhausted: AtomicBool,
}

/// A captured stripe stamp. Copy — cheap to take before/after awaits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stamp {
    pub(crate) stripe: usize,
    pub(crate) raw: u64,
}

impl Stamp {
    fn rev(self) -> u64 {
        self.raw >> ACTIVE_BITS
    }
    fn active(self) -> u64 {
        self.raw & ACTIVE_MASK
    }
}

/// Striped revision state shared by [`super::MemBufferState`].
pub(crate) struct StripedRevisions {
    stripes: Vec<StripeState>,
    hasher: THasher,
}

impl StripedRevisions {
    pub(crate) fn new() -> Self {
        Self {
            stripes: (0..STRIPES).map(|_| StripeState::default()).collect(),
            hasher: THasher::default(),
        }
    }

    /// Workspace-hash routing: `THasher(key) % STRIPES`.
    pub(crate) fn stripe_of(&self, key: &RecordKey) -> usize {
        let h = self.hasher.hash_one(key);
        h as usize % STRIPES
    }

    /// Capture the current packed stamp for `key`'s stripe (`Acquire`).
    pub(crate) fn stamp(&self, key: &RecordKey) -> Stamp {
        let stripe = self.stripe_of(key);
        Stamp {
            stripe,
            raw: self.stripes[stripe].packed.load(Ordering::Acquire),
        }
    }

    pub(super) fn is_exhausted(&self, stripe: usize) -> bool {
        self.stripes[stripe].exhausted.load(Ordering::Acquire)
    }

    /// Full acceptance check for a tagged cache entry
    /// `{entry_rev, Slot}`: the stripe stamp must be unchanged between the
    /// two captures (`s0 == s1`), no writer may be active at `s1`, the
    /// entry's tag must equal the current revision, and the stripe must not
    /// be exhausted. Any single failure rejects the entry (fail-closed).
    pub(crate) fn acceptable(&self, s0: Stamp, s1: Stamp, entry_rev: u64) -> bool {
        if s0.stripe != s1.stripe {
            // Same key always maps to the same stripe; this is defensive.
            return false;
        }
        if self.is_exhausted(s1.stripe) {
            return false;
        }
        s0.raw == s1.raw && s1.active() == 0 && s1.rev() == entry_rev
    }

    /// The revision field of a raw packed stamp — used to tag clean fills.
    pub(super) fn rev_of_raw(&self, raw: u64) -> u64 {
        raw >> ACTIVE_BITS
    }

    /// Begin a writer epoch: CAS `{rev+1, active+1}`. The returned guard
    /// performs the matching `{rev+1, active-1}` end-CAS on `Drop` (covering
    /// panics and task cancellation) — the revision bumps on BOTH edges.
    ///
    /// Exhaustion is fail-closed with no wrapping arithmetic:
    ///
    /// * the revision field is checked against the FIELD width (`rev <
    ///   REV_MAX`), not `u64::checked_add` — the field lives in the high
    ///   32 bits of the packed word, so it exhausts at 2^32-1, far below
    ///   `u64::MAX`; at [`REV_MAX`] it saturates there and the stripe is
    ///   latched exhausted (caching disabled for it permanently);
    /// * the active-writer field is likewise checked against its field
    ///   width: at [`ACTIVE_MASK`] the stripe is latched exhausted and the
    ///   guard is returned *uncounted* (its `end` will not decrement the
    ///   count) instead of overflowing.
    pub(crate) fn begin(&self, key: &RecordKey) -> StripeGuard<'_> {
        let stripe = self.stripe_of(key);
        let packed = &self.stripes[stripe].packed;
        loop {
            let cur = packed.load(Ordering::Acquire);
            let rev = cur >> ACTIVE_BITS;
            let active = cur & ACTIVE_MASK;
            // Revision: explicit FIELD-WIDTH check (`rev < REV_MAX`). A
            // `checked_add` on the extracted value would only fail at
            // u64::MAX — it can never see the packed field's real maximum.
            let next_rev = if rev < REV_MAX {
                rev + 1
            } else {
                self.stripes[stripe]
                    .exhausted
                    .store(true, Ordering::Release);
                REV_MAX
            };
            // Active: same field-width check; at the max, latch exhausted
            // and return an uncounted guard rather than overflow.
            let (next_active, counted) = if active < ACTIVE_MASK {
                (active + 1, true)
            } else {
                self.stripes[stripe]
                    .exhausted
                    .store(true, Ordering::Release);
                (active, false)
            };
            let next = (next_rev << ACTIVE_BITS) | next_active;
            if packed
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return StripeGuard {
                    stripes: self,
                    stripe,
                    counted,
                };
            }
        }
    }

    /// Test seam: force the stripe for `key` into the exhausted state
    /// (fail-closed caching disabled), so regression tests can prove the
    /// exhaustion path without spinning 2^32 times.
    #[cfg(test)]
    pub(crate) fn force_exhaust_for_test(&self, key: &RecordKey) {
        let stripe = self.stripe_of(key);
        let packed = &self.stripes[stripe].packed;
        self.stripes[stripe]
            .exhausted
            .store(true, Ordering::Release);
        // Pin the revision field at REV_MAX so any begin() also re-observes
        // the exhaustion boundary (revision saturates; it never wraps).
        let cur = packed.load(Ordering::Acquire);
        let _ = packed.compare_exchange(
            cur,
            (REV_MAX << ACTIVE_BITS) | (cur & ACTIVE_MASK),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Test seam: raw packed word for `key`'s stripe
    /// (`{revision << 32 | active_count}`).
    #[cfg(test)]
    pub(crate) fn packed_for_test(&self, key: &RecordKey) -> u64 {
        self.stripes[self.stripe_of(key)]
            .packed
            .load(Ordering::Acquire)
    }

    /// Test seam: overwrite the raw packed word for `key`'s stripe, so
    /// regression tests can place the stripe at an exact near-exhaustion
    /// boundary (revision or active field) without spinning billions of
    /// CAS iterations.
    #[cfg(test)]
    pub(crate) fn set_packed_for_test(&self, key: &RecordKey, raw: u64) {
        let stripe = self.stripe_of(key);
        let packed = &self.stripes[stripe].packed;
        let cur = packed.load(Ordering::Acquire);
        let _ = packed.compare_exchange(cur, raw, Ordering::AcqRel, Ordering::Acquire);
    }

    /// Test seam: exhaustion latch for `key`'s stripe.
    #[cfg(test)]
    pub(crate) fn is_exhausted_for_test(&self, key: &RecordKey) -> bool {
        self.stripes[self.stripe_of(key)]
            .exhausted
            .load(Ordering::Acquire)
    }
}

/// RAII writer epoch: the begin-CAS happened in [`StripedRevisions::begin`];
/// `Drop` performs the end-CAS `{rev+1, active-1}` (revision bumps on both
/// edges), so a panic or task cancellation (guard dropped mid-`.await`)
/// can never leak an active count. `counted` is `false` only for the
/// uncounted guard returned when the active field was already at max at
/// `begin` — that guard's `end` bumps the revision but leaves the count
/// untouched (no underflow).
pub(crate) struct StripeGuard<'a> {
    stripes: &'a StripedRevisions,
    stripe: usize,
    counted: bool,
}

impl StripeGuard<'_> {
    fn end(&mut self) {
        let packed = &self.stripes.stripes[self.stripe].packed;
        loop {
            let cur = packed.load(Ordering::Acquire);
            let rev = cur >> ACTIVE_BITS;
            let active = cur & ACTIVE_MASK;
            // Revision: explicit FIELD-WIDTH check, same fail-closed rule
            // as `begin` (a checked_add on the extracted value could never
            // observe the packed field's 32-bit maximum).
            let next_rev = if rev < REV_MAX {
                rev + 1
            } else {
                self.stripes.stripes[self.stripe]
                    .exhausted
                    .store(true, Ordering::Release);
                REV_MAX
            };
            // Active: balanced FIELD-BOUNDED decrement (only for a counted
            // guard); a non-counted guard leaves the field untouched.
            let next_active = if self.counted {
                if active > 0 {
                    active - 1
                } else {
                    debug_assert!(false, "end without matching begin");
                    active
                }
            } else {
                active
            };
            let next = (next_rev << ACTIVE_BITS) | next_active;
            if packed
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
    }
}

impl Drop for StripeGuard<'_> {
    fn drop(&mut self) {
        self.end();
    }
}
