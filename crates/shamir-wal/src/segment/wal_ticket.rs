use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use shamir_storage::error::DbError;
use tokio::sync::Notify;

/// Observable physical progress of a WAL frame. WRITTEN also remains the
/// final state when the frame was written but its requested fsync failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum WalTicketState {
    Pending,
    Written,
    Acked,
    Failed,
    Unknown,
}

/// Typed completion outcomes for the additive ticketed API. Legacy entry
/// points retain DbResult and their existing DbError::Storage match shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WalAppendError {
    NotWritten,
    WrittenNotDurable,
    Unknown,
    Indeterminate,
}

impl fmt::Display for WalAppendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotWritten => "wal frame not written",
            Self::WrittenNotDurable => "wal frame written but not durable",
            Self::Unknown => "wal frame outcome unknown; reopen WAL before appending",
            Self::Indeterminate => "wal group indeterminate; reopen WAL before appending",
        })
    }
}

impl std::error::Error for WalAppendError {}

impl From<WalAppendError> for DbError {
    /// Legacy completion conversion (RV-WAL-C55C2D47): deliberately returns
    /// `DbError::Storage` with the fixed, path-free outcome text of
    /// [`WalAppendError`] and DISCARDS the underlying sink cause. This
    /// preserves the classification/match shape of the legacy `DbResult`
    /// entry points and keeps WAL file paths out of client-visible errors.
    /// Callers that need the original cause use the ticketed API and
    /// [`WalTicket::failure_cause`], which retains the `Arc<DbError>`.
    fn from(error: WalAppendError) -> Self {
        Self::Storage(error.to_string())
    }
}

/// One allocation replaces the former private waiter. The caller may retain
/// this ticket after dropping its completion future; cancellation is not undo.
pub struct WalTicket {
    state: AtomicU8,
    done: AtomicBool,
    /// Underlying sink failure cause (plan P1.5): `DbError` is not `Clone`,
    /// so one window's failure is shared as an `Arc` and published BEFORE any
    /// `done` store — a waiter observing a terminal state (Acquire) also
    /// observes the cause.
    cause: OnceLock<Arc<DbError>>,
    pub(crate) notify: Notify,
}

impl WalTicket {
    pub(crate) fn new() -> Self {
        Self {
            state: AtomicU8::new(WalTicketState::Pending as u8),
            done: AtomicBool::new(false),
            cause: OnceLock::new(),
            notify: Notify::new(),
        }
    }

    /// Construct a detached ticket fixture for cross-crate state-machine tests.
    /// This ticket is not associated with any WAL frame and must never be used
    /// to infer physical progress of an actual append. Mem fault knobs are
    /// crate-private and cfg(test), unavailable to downstream unit tests.
    #[doc(hidden)]
    pub fn detached_for_test(state: WalTicketState) -> Self {
        Self {
            state: AtomicU8::new(state as u8),
            done: AtomicBool::new(state != WalTicketState::Pending),
            cause: OnceLock::new(),
            notify: Notify::new(),
        }
    }

    #[doc(hidden)]
    pub fn complete_detached_for_test(&self, state: WalTicketState) {
        self.complete(state);
    }

    pub fn state(&self) -> WalTicketState {
        match self.state.load(Ordering::Acquire) {
            0 => WalTicketState::Pending,
            1 => WalTicketState::Written,
            2 => WalTicketState::Acked,
            3 => WalTicketState::Failed,
            4 => WalTicketState::Unknown,
            _ => unreachable!("invalid WAL ticket state"),
        }
    }

    pub(crate) fn written(&self) {
        self.state
            .store(WalTicketState::Written as u8, Ordering::Release);
    }

    pub(crate) fn complete(&self, state: WalTicketState) {
        // append_many shares one ticket among its physical frames. Completion
        // is idempotent and must not wake repeatedly for duplicate metadata.
        if !self.done.load(Ordering::Acquire) {
            self.state.store(state as u8, Ordering::Release);
            self.done.store(true, Ordering::Release);
            // Wake EVERY registered awaiter: the batched API (`append_many`)
            // shares one ticket among several awaiters, and `notify_one`
            // would wake only one of them. ALSO store one permit for a
            // late registrant (poll-based completion futures create their
            // `Notified` after `complete`, and `notify_waiters` stores no
            // permit); every awaiter re-reads `done` after a wake.
            self.notify.notify_waiters();
            self.notify.notify_one();
        }
    }

    /// Preserve the underlying sink failure for this frame. Call BEFORE any
    /// `complete`/`written` publication of a terminal state.
    pub(crate) fn set_cause(&self, cause: Arc<DbError>) {
        let _ = self.cause.set(cause);
    }

    /// The preserved underlying sink failure, if this frame failed.
    pub fn failure_cause(&self) -> Option<Arc<DbError>> {
        self.cause.get().cloned()
    }

    pub(crate) fn outcome(&self) -> Option<Result<(), WalAppendError>> {
        if !self.done.load(Ordering::Acquire) {
            return None;
        }
        Some(match self.state() {
            WalTicketState::Acked => Ok(()),
            WalTicketState::Failed => Err(WalAppendError::NotWritten),
            WalTicketState::Written => Err(WalAppendError::WrittenNotDurable),
            WalTicketState::Unknown => Err(WalAppendError::Unknown),
            WalTicketState::Pending => unreachable!("completed pending WAL ticket"),
        })
    }
}
