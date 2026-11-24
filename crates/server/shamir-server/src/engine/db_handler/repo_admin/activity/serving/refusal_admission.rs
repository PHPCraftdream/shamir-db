//! The bounded, non-waiting admission decision of the detached audit queue:
//! one share slot of the principal and one global slot are taken as ONE
//! bucket-locked decision, or the event is refused. Child module of
//! `repo_audit` so it reaches the queue's private counters.

use std::sync::atomic::Ordering;
use std::sync::Arc;

#[cfg(test)]
use super::fire_share_decision_probe;
use super::{
    AuditEventClass, CancelAuditSlot, CancelRefusalAudit, PrincipalHeld, ADMISSION_RETRY_LIMIT,
    CANCEL_AUDIT_CAPACITY, CANCEL_AUDIT_OBSERVABILITY_CAPACITY,
    CANCEL_AUDIT_OBSERVABILITY_PRINCIPAL_SHARE, CANCEL_AUDIT_PRINCIPAL_SHARE, CLOSED, COUNT_MASK,
    SATURATED,
};

#[derive(Clone, Copy)]
enum GlobalRefusal {
    Closed,
    Saturated,
    Full,
    ObservabilityFull,
    RetryLimit,
}

#[derive(Clone, Copy)]
enum AdmissionDecision {
    Reserved,
    ShareFull,
    Global(GlobalRefusal),
    RetryLimit,
}

/// Opaque refusal verdict: decided under the queue's guards, accounted and logged
/// by [`CancelRefusalAudit::log_refusal`] after the caller dropped them.
#[derive(Clone, Copy)]
pub(super) struct Refusal(AdmissionDecision);

impl CancelRefusalAudit {
    /// Reserve one slot of `principal`'s share (for `Observability` also of its
    /// observability share) AND one global slot as ONE bounded, non-waiting
    /// decision, or refuse.
    ///
    /// The share tests, the global reservation and the increment run under the
    /// SAME bucket lock (see [`Self::reserve_share_and_global`]), so the global
    /// slot is taken only when the share still has room: a doomed request from
    /// a principal already at its share never consumes (and never refuses
    /// another principal) a global slot. `reserve_global_slot` is a lock-free
    /// CAS loop, so it takes no second lock and cannot block inside the closure.
    ///
    /// `Observability` is additionally confined to
    /// [`CANCEL_AUDIT_OBSERVABILITY_PRINCIPAL_SHARE`] of the principal's share
    /// and [`CANCEL_AUDIT_OBSERVABILITY_CAPACITY`] of the global cap; `Security`
    /// may use the full share and the full cap, so the reserved headroom stays
    /// available to it when observability is saturated.
    ///
    /// A principal entry therefore always implies a reserved slot, so the map
    /// can never hold more live entries than `CANCEL_AUDIT_CAPACITY` (the bound
    /// its doc states).
    #[cfg(test)]
    pub(super) fn try_reserve(
        self: &Arc<Self>,
        principal: &str,
        class: AuditEventClass,
    ) -> Option<CancelAuditSlot> {
        match self.reserve_verdict(principal, class) {
            Ok(slot) => Some(slot),
            Err(verdict) => {
                self.log_refusal(verdict, class);
                None
            }
        }
    }

    /// Decide admission without accounting or logging; callers holding the
    /// spawn gate must release it before processing a refusal.
    pub(super) fn reserve_verdict(
        self: &Arc<Self>,
        principal: &str,
        class: AuditEventClass,
    ) -> Result<CancelAuditSlot, Refusal> {
        if self.share_is_full(principal, class) {
            return Err(Refusal(AdmissionDecision::ShareFull));
        }
        #[cfg(test)]
        fire_share_decision_probe();
        match self.reserve_share_and_global(principal, class) {
            AdmissionDecision::Reserved => Ok(CancelAuditSlot {
                queue: Arc::clone(self),
                principal: principal.to_owned(),
                class,
                appended: false,
            }),
            verdict => Err(Refusal(verdict)),
        }
    }

    pub(super) fn log_refusal(&self, verdict: Refusal, class: AuditEventClass) {
        match verdict.0 {
            AdmissionDecision::ShareFull => self.refuse_share_full(class),
            AdmissionDecision::Global(reason) => self.refuse_global(reason, class),
            AdmissionDecision::RetryLimit => self.refuse_retry_limit(class),
            AdmissionDecision::Reserved => unreachable!("reserved is not a refusal"),
        }
    }

    /// Read-only share test: creates no entry and takes no slot.
    fn share_is_full(&self, principal: &str, class: AuditEventClass) -> bool {
        match self.principals.try_entry(principal.to_owned()) {
            Some(scc::hash_map::Entry::Occupied(entry)) => entry.get().is_full(class),
            Some(scc::hash_map::Entry::Vacant(_)) | None => false,
        }
    }

    /// Take one slot of `principal`'s share AND one global slot in ONE
    /// bucket-locked decision, or refuse without waiting. `try_entry` returns
    /// immediately when the scc 3.8 bucket is busy; each occupied/vacant
    /// decision is one bounded entry operation.
    fn reserve_share_and_global(
        &self,
        principal: &str,
        class: AuditEventClass,
    ) -> AdmissionDecision {
        for _ in 0..ADMISSION_RETRY_LIMIT {
            match self.principals.try_entry(principal.to_owned()) {
                Some(scc::hash_map::Entry::Occupied(mut entry)) => {
                    let held = entry.get_mut();
                    if held.is_full(class) {
                        return AdmissionDecision::ShareFull;
                    }
                    match self.reserve_global_slot(class) {
                        Ok(()) => {
                            held.total += 1;
                            if class == AuditEventClass::Observability {
                                held.observability += 1;
                            }
                            return AdmissionDecision::Reserved;
                        }
                        Err(reason) => return AdmissionDecision::Global(reason),
                    }
                }
                Some(scc::hash_map::Entry::Vacant(entry)) => {
                    if let Err(reason) = self.reserve_global_slot(class) {
                        return AdmissionDecision::Global(reason);
                    }
                    let held = PrincipalHeld {
                        total: 1,
                        observability: usize::from(class == AuditEventClass::Observability),
                    };
                    entry.insert_entry(held);
                    return AdmissionDecision::Reserved;
                }
                None => std::hint::spin_loop(),
            }
        }
        AdmissionDecision::RetryLimit
    }

    fn refuse_retry_limit(&self, class: AuditEventClass) {
        let dropped = self.record_drop(class);
        tracing::warn!(
            class = class.label(),
            retries = ADMISSION_RETRY_LIMIT,
            dropped,
            "cancel refusal audit admission retry limit reached; dropping event"
        );
    }

    fn refuse_global(&self, reason: GlobalRefusal, class: AuditEventClass) {
        match reason {
            GlobalRefusal::Closed => {
                let dropped = self.record_drop(class);
                tracing::debug!(
                    class = class.label(),
                    dropped,
                    "cancel refusal audit dropped: admission closed"
                );
            }
            GlobalRefusal::Full => {
                let dropped = self.record_drop(class);
                tracing::warn!(
                    class = class.label(),
                    capacity = CANCEL_AUDIT_CAPACITY,
                    dropped,
                    "cancel refusal audit queue full; dropping events"
                );
            }
            GlobalRefusal::ObservabilityFull => {
                let dropped = self.record_drop(class);
                tracing::warn!(
                    class = class.label(),
                    capacity = CANCEL_AUDIT_OBSERVABILITY_CAPACITY,
                    dropped,
                    "observability audit quota full; dropping event"
                );
            }
            GlobalRefusal::Saturated => {
                self.record_drop(class);
            }
            GlobalRefusal::RetryLimit => {
                self.refuse_retry_limit(class);
            }
        }
    }

    /// Record one refused submission: exactly one drop and one warn line, the
    /// same accounting for the read-only pre-check and for the decision.
    fn refuse_share_full(&self, class: AuditEventClass) {
        let dropped = self.record_drop(class);
        tracing::warn!(
            class = class.label(),
            share = CANCEL_AUDIT_PRINCIPAL_SHARE,
            observability_share = CANCEL_AUDIT_OBSERVABILITY_PRINCIPAL_SHARE,
            dropped,
            "cancel refusal audit principal share full; dropping event"
        );
    }

    /// Take one GLOBAL slot of `class`, or refuse: the observability quota
    /// (for `Observability` only), admission closed, capacity reached, or an
    /// earlier rejection in the same saturation episode already latched it.
    /// The count grows only here.
    fn reserve_global_slot(&self, class: AuditEventClass) -> Result<(), GlobalRefusal> {
        if class == AuditEventClass::Observability {
            let mut held = self.observability.load(Ordering::Acquire);
            let mut reserved = false;
            for _ in 0..ADMISSION_RETRY_LIMIT {
                if held >= CANCEL_AUDIT_OBSERVABILITY_CAPACITY {
                    return Err(GlobalRefusal::ObservabilityFull);
                }
                match self.observability.compare_exchange_weak(
                    held,
                    held + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        reserved = true;
                        break;
                    }
                    Err(current) => held = current,
                }
            }
            if !reserved {
                return Err(GlobalRefusal::RetryLimit);
            }
        }
        let result = self.reserve_state_slot();
        if result.is_err() {
            self.release_observability(class);
        }
        result
    }

    /// The total-count half of [`Self::reserve_global_slot`].
    fn reserve_state_slot(&self) -> Result<(), GlobalRefusal> {
        let mut state = self.state.load(Ordering::Acquire);
        for _ in 0..ADMISSION_RETRY_LIMIT {
            if state & CLOSED != 0 {
                return Err(GlobalRefusal::Closed);
            }
            let full = state & COUNT_MASK == CANCEL_AUDIT_CAPACITY;
            if full && state & SATURATED != 0 {
                return Err(GlobalRefusal::Saturated);
            }
            let next = if full { state | SATURATED } else { state + 1 };
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) if full => return Err(GlobalRefusal::Full),
                Ok(_) => return Ok(()),
                Err(current) => state = current,
            }
        }
        Err(GlobalRefusal::RetryLimit)
    }

    /// Give one GLOBAL slot back, clearing the saturation latch in the same
    /// transition. Every path that reserved a slot but produced no
    /// [`CancelAuditSlot`] calls this; the slot's own `Drop` does too.
    pub(super) fn release_global_slot(&self) {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            let next = (state - 1) & !SATURATED;
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    // An OPEN queue's drain waiter needs the zero crossing, not
                    // only the shutdown signal: `drain` loops on
                    // `idle.notified()` until the count reaches zero.
                    if next == CLOSED || next & COUNT_MASK == 0 {
                        self.idle.notify_waiters();
                    }
                    return;
                }
                Err(current) => state = current,
            }
        }
    }
}
