use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::{BudgetPin, LoginBudgetTable, Source};

const CAPACITY: usize = 1_024;
const BASE_SLOT_LIMIT: usize = 16;
const PARENT_SLOT_LIMIT: usize = 64;
const MID_SLOT_LIMIT: usize = 64;

/// Injectable monotonic time source.
pub trait LoginClock: Send + Sync {
    /// Current monotonic instant.
    fn now(&self) -> Instant;
}

struct SystemClock;
impl LoginClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Stable admission rejection categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    /// No remaining tier credit.
    RateLimited {
        /// Seconds until the next admission retry may succeed.
        retry_after_seconds: u32,
    },
    /// Global request-slot pool exhausted.
    ServerBusy,
    /// Per-source request slots exhausted.
    SourceSlots,
}

struct Slot {
    marker: Arc<AtomicBool>,
    source: Option<Source>,
}

struct State {
    slots: Vec<Slot>,
    arrival: LoginBudgetTable,
    credit: LoginBudgetTable,
}

/// Fixed-capacity admission slots and hierarchical rate budgets.
pub struct AdmissionState {
    clock: Arc<dyn LoginClock>,
    inner: Mutex<State>,
}

impl AdmissionState {
    /// Creates a bounded state with an injected clock.
    pub fn new(clock: Arc<dyn LoginClock>) -> Self {
        Self {
            clock,
            inner: Mutex::new(State {
                slots: (0..CAPACITY)
                    .map(|_| Slot {
                        marker: Arc::new(AtomicBool::new(false)),
                        source: None,
                    })
                    .collect(),
                arrival: LoginBudgetTable::new(Duration::from_secs(1)),
                credit: LoginBudgetTable::new(Duration::from_secs(60)),
            }),
        }
    }

    /// Creates a production state using the system monotonic clock.
    pub fn system() -> Self {
        Self::new(Arc::new(SystemClock))
    }

    /// Pins the resident arrival and credit buckets for pending work.
    pub async fn pin_source(&self, source: Source) -> (BudgetPin, BudgetPin) {
        let state = self.inner.lock().await;
        (state.arrival.pin(source), state.credit.pin(source))
    }

    /// Admits a slot without debiting rate budgets; test-only slot accounting seam.
    #[cfg(test)]
    pub(crate) async fn admit_slot_for_test(
        &self,
        source: Source,
    ) -> Result<InputPermit, AdmissionError> {
        let mut state = self.inner.lock().await;
        admit_slot(&mut state.slots, source)
    }

    /// Admits a request after debiting rate budgets and accounting live slots.
    pub async fn admit(&self, source: Source) -> Result<InputPermit, AdmissionError> {
        let mut state = self.inner.lock().await;
        // Keep clock access inside the bounded critical section to avoid stale debits under contention.
        let now = self.clock.now();
        let arrival = state.arrival.debit(source, now);
        match arrival {
            Err(denied) => {
                return Err(AdmissionError::RateLimited {
                    retry_after_seconds: denied.retry_after_seconds,
                });
            }
            Ok(super::DebitOutcome::Dropped(_)) => {
                super::super::metrics::record_budget_drop("arrival_table_full");
            }
            Ok(super::DebitOutcome::Charged) => {}
        }
        let credit = state.credit.debit(source, now);
        match credit {
            Err(denied) => {
                return Err(AdmissionError::RateLimited {
                    retry_after_seconds: denied.retry_after_seconds,
                });
            }
            Ok(super::DebitOutcome::Dropped(_)) => {
                super::super::metrics::record_budget_drop("credit_table_full");
            }
            Ok(super::DebitOutcome::Charged) => {}
        }
        let mut permit = admit_slot(&mut state.slots, source)?;
        permit._arrival_pin = Some(state.arrival.pin(source));
        permit._credit_pin = Some(state.credit.pin(source));
        Ok(permit)
    }
}

fn admit_slot(slots: &mut [Slot], source: Source) -> Result<InputPermit, AdmissionError> {
    if slots
        .iter()
        .filter(|slot| slot.marker.load(Ordering::Acquire))
        .count()
        >= CAPACITY
    {
        return Err(AdmissionError::ServerBusy);
    }
    let tiers = source.tiers();
    let limits = [BASE_SLOT_LIMIT, PARENT_SLOT_LIMIT, MID_SLOT_LIMIT];
    for (index, (tier, limit)) in tiers.into_iter().zip(limits).enumerate() {
        if index > 0 && tiers[..index].contains(&tier) {
            continue;
        }
        let count = slots
            .iter()
            .filter(|slot| {
                slot.marker.load(Ordering::Acquire)
                    && slot
                        .source
                        .is_some_and(|active| active.tiers().contains(&tier))
            })
            .count();
        if count >= limit {
            return Err(AdmissionError::SourceSlots);
        }
    }
    let Some(slot) = slots
        .iter_mut()
        .find(|slot| !slot.marker.load(Ordering::Acquire))
    else {
        return Err(AdmissionError::ServerBusy);
    };
    slot.source = Some(source);
    slot.marker.store(true, Ordering::Release);
    Ok(InputPermit {
        marker: slot.marker.clone(),
        source,
        _arrival_pin: None,
        _credit_pin: None,
    })
}

/// RAII input capacity reservation released exactly once on drop.
pub struct InputPermit {
    marker: Arc<AtomicBool>,
    source: Source,
    _arrival_pin: Option<BudgetPin>,
    _credit_pin: Option<BudgetPin>,
}

impl InputPermit {
    /// Effective source for this admitted request.
    pub const fn source(&self) -> Source {
        self.source
    }
}

impl Drop for InputPermit {
    fn drop(&mut self) {
        self.marker.store(false, Ordering::Release);
    }
}
