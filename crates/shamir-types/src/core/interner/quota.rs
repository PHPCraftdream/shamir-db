use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InternerQuotaKind {
    Names,
    Bytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InternerQuota {
    pub max_names: u64,
    pub max_bytes: u64,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum InternerError {
    #[error(
        "interner quota exceeded for {label}: {kind:?} limit {limit} — raise SHAMIR_INTERNER_MAX_NAMES / SHAMIR_INTERNER_MAX_NAME_BYTES"
    )]
    QuotaExceeded {
        label: String,
        kind: InternerQuotaKind,
        limit: u64,
    },
    #[error("interner id collision at {id}")]
    IdCollision { id: u64 },
    #[error("interner name {name:?} is already assigned to id {existing}, not {requested}")]
    NameConflict {
        name: String,
        existing: u64,
        requested: u64,
    },
    #[error("interner id {id} is already assigned to {existing:?}, not {requested:?}")]
    IdConflict {
        id: u64,
        existing: String,
        requested: String,
    },
    #[error("interner id 0 is reserved")]
    ReservedId,
}

/// Internal, label-free rejection from `QuotaCounters::reserve` — the
/// public [`InternerError::QuotaExceeded`] (with the repo label) is
/// constructed at the `Interner` call sites so the hot path never clones
/// the label.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct QuotaRejection {
    pub(crate) kind: InternerQuotaKind,
    pub(crate) limit: u64,
}

#[derive(Debug)]
pub(crate) struct QuotaCounters {
    names: AtomicU64,
    bytes: AtomicU64,
}

impl QuotaCounters {
    pub(crate) fn new(names: u64, bytes: u64) -> Self {
        Self {
            names: AtomicU64::new(names),
            bytes: AtomicU64::new(bytes),
        }
    }

    pub(crate) fn snapshot(&self) -> (u64, u64) {
        (
            self.names.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn reserve(&self, quota: InternerQuota, bytes: u64) -> Result<(), QuotaRejection> {
        reserve_one(&self.names, quota.max_names, 1, InternerQuotaKind::Names)?;
        if let Err(error) = reserve_one(
            &self.bytes,
            quota.max_bytes,
            bytes,
            InternerQuotaKind::Bytes,
        ) {
            self.names.fetch_sub(1, Ordering::Relaxed);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn refund(&self, bytes: u64) {
        self.names.fetch_sub(1, Ordering::Relaxed);
        self.bytes.fetch_sub(bytes, Ordering::Relaxed);
    }

    pub(crate) fn count_unmetered(&self, bytes: u64) {
        self.names.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
}

fn reserve_one(
    counter: &AtomicU64,
    limit: u64,
    amount: u64,
    kind: InternerQuotaKind,
) -> Result<(), QuotaRejection> {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current
            .checked_add(amount)
            .ok_or(QuotaRejection { kind, limit })?;
        if next > limit {
            return Err(QuotaRejection { kind, limit });
        }
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}
