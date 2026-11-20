use std::pin::pin;
use std::sync::atomic::{AtomicU8, Ordering};

use thiserror::Error;
use tokio::sync::Notify;

/// Typed marker for a request stopped by cooperative cancellation.
#[derive(Debug, Error)]
#[error("request cancelled")]
pub struct RequestCancelled;

/// The request state shared by the executor and the cancellation caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CancelState {
    Running,
    Committing,
    Done,
    Cancelled,
}

impl CancelState {
    fn from_word(word: u8) -> Self {
        match word {
            0 => Self::Running,
            1 => Self::Committing,
            2 => Self::Done,
            3 => Self::Cancelled,
            _ => unreachable!("invalid cancellation state"),
        }
    }
}

/// Why a request is currently parked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WaitState {
    None = 0,
    Lock = 1,
    Visibility = 2,
    Budget = 3,
}

impl WaitState {
    fn from_word(word: u8) -> Self {
        match word {
            0 => Self::None,
            1 => Self::Lock,
            2 => Self::Visibility,
            3 => Self::Budget,
            _ => unreachable!("invalid wait state"),
        }
    }
}

/// Cooperative request cancellation and commit arbitration.
#[derive(Debug, Default)]
pub struct CancelHandle {
    state: AtomicU8,
    waiting: AtomicU8,
    cancelled: Notify,
}

impl CancelHandle {
    /// Cancel a running request, or return the state that refused cancellation.
    pub fn try_cancel(&self) -> Result<(), CancelState> {
        self.transition(CancelState::Running, CancelState::Cancelled)?;
        self.cancelled.notify_waiters();
        Ok(())
    }

    /// Claim the right to commit, before entering the durable phase.
    pub fn try_commit(&self) -> bool {
        self.transition(CancelState::Running, CancelState::Committing)
            .is_ok()
    }

    /// Mark execution complete without overwriting a winning cancellation.
    pub fn finish(&self) -> bool {
        let mut state = self.state();
        loop {
            match state {
                CancelState::Done => return true,
                CancelState::Cancelled => return false,
                CancelState::Running | CancelState::Committing => {
                    match self.transition(state, CancelState::Done) {
                        Ok(()) => return true,
                        Err(actual) => state = actual,
                    }
                }
            }
        }
    }

    fn transition(&self, from: CancelState, to: CancelState) -> Result<(), CancelState> {
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(CancelState::from_word)
    }

    pub fn state(&self) -> CancelState {
        CancelState::from_word(self.state.load(Ordering::Acquire))
    }

    pub fn is_cancelled(&self) -> bool {
        self.state() == CancelState::Cancelled
    }

    /// Wait for a winning cancellation, including one issued before this call.
    ///
    /// cancel-safe: yes — dropping this future only unregisters its waiter.
    pub async fn wait_cancelled(&self) {
        loop {
            let mut notified = pin!(self.cancelled.notified());
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    /// Snapshot the current wait reason.
    pub fn waiting(&self) -> WaitState {
        WaitState::from_word(self.waiting.load(Ordering::Acquire))
    }

    /// Publish a wait until the returned guard is dropped.
    ///
    /// The request executor owns its parks and performs them serially. `None`
    /// and an overlapping park attempt both return a no-op guard.
    pub fn park(&self, state: WaitState) -> WaitGuard<'_> {
        let owns_wait = state != WaitState::None
            && self
                .waiting
                .compare_exchange(
                    WaitState::None as u8,
                    state as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok();
        WaitGuard {
            waiting: owns_wait.then_some(&self.waiting),
        }
    }
}

/// Keeps an acquired wait reason published for the lifetime of a park.
#[must_use = "keep the guard alive while parked"]
pub struct WaitGuard<'a> {
    waiting: Option<&'a AtomicU8>,
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        if let Some(waiting) = self.waiting {
            waiting.store(WaitState::None as u8, Ordering::Release);
        }
    }
}
