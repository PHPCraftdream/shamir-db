//! Per-repo online-backup capture/seal facade (R2a step 1d).
//!
//! Design: `docs/dev-artifacts/design/online-backup.md`, section
//! "Step 1 findings (2026-11-17), revised into the settled capture/seal
//! design" — hazard-1 remedy (drain with NO repo write permit held, pause
//! over the drainer's `replaying` lock), section 2 (iii) order, section 3
//! facade mapping (`capture_point` / `seal_point`). The R2a-3 server
//! adapter calls these; nothing here touches shamir-sync/shamir-server.
//!
//! Sequences (lock order: gate close FIRST, pause SECOND — never reverse):
//!
//! * [`RepoInstance::capture_point`] (step B): busy probe → close gate +
//!   wait zero leases (bounded; owner RAII, created before any suspension
//!   point) → drain to settled with NO repo write permit held (fail-closed
//!   on failure/unsettled) → `flush_all_history` → read
//!   `RepoTxGate::last_committed` → owner drop reopens.
//! * [`RepoInstance::seal_point`] (step E, up to the hold): same up to the
//!   settled drain → acquire the drain pause → recheck pending work under
//!   the pause (non-empty fails closed) → flush history → sample `V*` →
//!   reopen gate → return the [`SealHold`] owning the pause. Writers run
//!   again after return, but nothing is applied to the stores (drain is
//!   blocked; `offer` drops above the window high-watermark and the
//!   gap-reseed path recovers entries after the hold drops) until the hold
//!   drops.
//!
//! Position source (verified, R2a-1d): `RepoTxGate::last_committed`
//! (`shamir-tx/src/gate/repo_tx_gate/core.rs:54`) is the max
//! assigned-and-visible commit version; after gate close + zero leases +
//! settled drain it equals `V*` (the durable watermark is asserted equal
//! as part of the settle/recheck check). `pub` only as far as the R2a-3
//! adapter provably needs.

use std::time::{Duration, Instant};

use shamir_storage::error::{DbError, DbResult};

use crate::tx::commit::pipeline::admission::close_owner::CommitAdmissionCloseOwner;
use crate::tx::commit::pipeline::admission::error::CommitAdmissionError;
use crate::tx::drain::drain_pause::DrainPauseGuard;
use crate::tx::drain::truncation_pin_guard::TruncationPinGuard;

use super::RepoInstance;

/// Bounded zero-lease wait for the capture/seal barriers. Matches the
/// leader reopen cap in spirit (`ADMISSION_REOPEN_WAIT_CAP`,
/// `COMMIT_QUIESCE_TIMEOUT`); lifted to `shamir-tunables` when a slot fits.
const BARRIER_ZERO_LEASE_CAP: Duration = Duration::from_secs(15);

/// Typed barrier failures. All variants leave the repository unchanged:
/// the close owner (RAII) reopens the gate and the pause guard releases
/// the drain on every error path, timeout, cancellation or unwind.
#[derive(Debug, thiserror::Error)]
pub enum OnlineBackupError {
    /// Another barrier (close owner) or a live seal hold owns this repo.
    /// Nothing was mutated.
    #[error("an online-backup barrier or seal hold is already active on this repository")]
    Busy,
    /// The gate was closed past the barrier cap while leases stayed active
    /// (mapped from `CommitAdmissionError::CloseTimeout`).
    #[error("commit admission leases did not reach zero within the barrier cap")]
    QuiesceTimeout,
    /// Admission refused as closed (mapped from
    /// `CommitAdmissionError::Closed`) — a concurrent barrier won the race.
    #[error("commit admission is closed; another barrier holds the repository")]
    AdmissionClosed,
    /// The settled drain check failed: pending window entries, unresolved
    /// WAL versions or `durable != last_committed` remained after the
    /// ordinary drain. Fails closed — no position is reported.
    #[error("drain did not settle before the barrier; refusing to report a position")]
    DrainUnsettled,
    /// The under-pause recheck found pending work (a non-empty recheck
    /// fails closed per design section 2 (iii)).
    #[error("pending drain work remained under the seal pause; failing closed")]
    RecheckPending,
    /// A seal hold already owns the drain pause; a second barrier may not
    /// enter.
    #[error("a seal hold already owns the drain pause on this repository")]
    HoldActive,
    /// Underlying engine failure (drain I/O, flush). Fails closed.
    #[error(transparent)]
    Engine(#[from] DbError),
}

fn map_admission(error: CommitAdmissionError) -> OnlineBackupError {
    match error {
        CommitAdmissionError::Busy => OnlineBackupError::Busy,
        CommitAdmissionError::Closed => OnlineBackupError::AdmissionClosed,
        CommitAdmissionError::CloseTimeout => OnlineBackupError::QuiesceTimeout,
        CommitAdmissionError::Overflow => OnlineBackupError::Engine(DbError::Internal(
            "commit admission lease count overflow".into(),
        )),
    }
}

/// Owns the drain pause from `seal_point` until drop. `Send + 'static`, so
/// the R2a-3 server adapter can hold it across the seal re-copy. While
/// alive: commits may run (the gate is already reopened), but the drainer
/// applies nothing — copied store files provably contain nothing above
/// `V*` (design section 2 (iii)). The re-copy duration cap policy lives
/// with the caller; the hold only reports how long it has been owned.
pub struct SealHold {
    /// The sampled consistency position `V*`.
    position: u64,
    /// Monotonic instant the hold was created (cap policy input).
    since: Instant,
    /// The pause guard; dropped exactly once, with the hold.
    _pause: DrainPauseGuard,
}

impl SealHold {
    /// The sealed consistency position (`V*`, inclusive).
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Monotonic instant the hold was created (held-duration = `now -
    /// this`; the cap policy lives with the caller).
    pub fn held_since(&self) -> Instant {
        self.since
    }
}

/// Test seam for the later cancellation tests (R2a-1e/1f): a hook fired at
/// each named barrier step so a test can drop the capture/seal future
/// exactly between steps. `test-util`-gated like the engine's other
/// cross-crate seams (`arm_sync_wal_fault`); production never sees it.
#[cfg(any(test, feature = "test-util"))]
static ONLINE_BACKUP_STEP_HOOK: std::sync::OnceLock<fn(&'static str)> = std::sync::OnceLock::new();

impl RepoInstance {
    /// Install the barrier step hook (test-only; first installer wins).
    /// Associated with the publicly reachable `RepoInstance` so the seam
    /// stays live for shamir-db / e2e dev-dependency tests.
    #[cfg(any(test, feature = "test-util"))]
    #[doc(hidden)]
    pub fn set_online_backup_step_hook(hook: fn(&'static str)) {
        let _ = ONLINE_BACKUP_STEP_HOOK.set(hook);
    }

    #[cfg(any(test, feature = "test-util"))]
    fn fire_step(step: &'static str) {
        if let Some(hook) = ONLINE_BACKUP_STEP_HOOK.get() {
            hook(step);
        }
    }

    /// Step B flush barrier: close admission, quiesce leases, drain to
    /// settled, flush history, report the consistency position. Does NOT
    /// acquire a truncation pin (pins are held from step A by the caller,
    /// via [`RepoInstance::acquire_truncation_pin`]).
    ///
    /// cancel-safe as a whole: the close owner is created before the first
    /// suspension point and reopens on drop — error, timeout, cancellation
    /// or unwind always leaves the gate open and the drain unpaused.
    pub async fn capture_point(&self) -> Result<u64, OnlineBackupError> {
        if self.core.drainer().pause_held() {
            return Err(OnlineBackupError::HoldActive);
        }
        // Lock order: gate close FIRST (the pause is never taken here).
        let owner = self.barrier_owner()?;
        let outcome: Result<u64, OnlineBackupError> = Box::pin(async {
            #[cfg(any(test, feature = "test-util"))]
            Self::fire_step("after_close");
            owner
                .wait_zero(BARRIER_ZERO_LEASE_CAP)
                .await
                .map_err(map_admission)?;
            #[cfg(any(test, feature = "test-util"))]
            Self::fire_step("after_quiesce");
            // NO repo write permit is held here — the ordinary drain paths
            // acquire their own domain permits (hazard 1 is self-deadlock
            // by the CALLER holding a permit; this caller holds none).
            self.drain_to_settled().await?;
            self.core.flush_all_history().await?;
            let position = self
                .core
                .initialized_tx_gate()
                .map(|gate| gate.last_committed())
                .unwrap_or(0);
            Ok(position)
        })
        .await;
        drop(owner); // fail-closed reopen in every outcome
        outcome
    }

    /// Step E seal barrier: the design §2 (iii) order up to the hold —
    /// close gate → zero leases → settled drain (no permit held) → pause
    /// guard → non-empty-recheck fails closed → flush history → sample
    /// `V*` → reopen gate → return the `SealHold`. Capture and seal are
    /// mutually exclusive with each other and with any live hold (typed
    /// busy errors).
    pub async fn seal_point(&self) -> Result<SealHold, OnlineBackupError> {
        if self.core.drainer().pause_held() {
            return Err(OnlineBackupError::HoldActive);
        }
        // Lock order: gate close FIRST, pause SECOND (below).
        let owner = self.barrier_owner()?;
        let outcome: Result<(u64, DrainPauseGuard), OnlineBackupError> = Box::pin(async {
            #[cfg(any(test, feature = "test-util"))]
            Self::fire_step("after_close");
            owner
                .wait_zero(BARRIER_ZERO_LEASE_CAP)
                .await
                .map_err(map_admission)?;
            #[cfg(any(test, feature = "test-util"))]
            Self::fire_step("after_quiesce");
            // The last drain_all pass held `replaying`, so any in-flight
            // background pass finished before it returned; draining BEFORE
            // pausing avoids the recursive `replaying` acquisition
            // (hazard 1).
            self.drain_to_settled().await?;
            #[cfg(any(test, feature = "test-util"))]
            Self::fire_step("after_drain");
            let guard = self
                .core
                .drainer()
                .pause()
                .acquire()
                .await
                .map_err(|_| OnlineBackupError::HoldActive)?;
            #[cfg(any(test, feature = "test-util"))]
            Self::fire_step("after_pause");
            // Recheck under the pause: nothing may be pending. Non-empty
            // fails closed; the guard drops on the error path, releasing
            // the pause.
            if !self.settled_under_pause() {
                return Err(OnlineBackupError::RecheckPending);
            }
            #[cfg(any(test, feature = "test-util"))]
            Self::fire_step("after_recheck");
            self.core.flush_all_history().await?;
            let position = self
                .core
                .initialized_tx_gate()
                .map(|gate| gate.last_committed())
                .unwrap_or(0);
            Ok((position, guard))
        })
        .await;
        drop(owner); // reopen the gate BEFORE the hold is handed out
        outcome.map(|(position, guard)| SealHold {
            position,
            since: Instant::now(),
            _pause: guard,
        })
    }

    /// Minimal public pin acquisition (R2a-3 facade): retain WAL history at
    /// `floor` until the returned guard drops (RAII, exactly once,
    /// including cancellation and unwind). Idempotent in the caller sense:
    /// each call adds one independent pin. No pin semantics changed.
    /// Capture/seal do NOT acquire pins themselves.
    pub async fn acquire_truncation_pin(&self, floor: u64) -> DbResult<TruncationPinGuard> {
        self.core.pin_wal_history(floor).await
    }

    /// Busy probe + close: the single-owner CAS decides; the owner is
    /// created synchronously (before any suspension point) so a cancelled
    /// barrier can never leak a closed gate.
    fn barrier_owner(&self) -> Result<CommitAdmissionCloseOwner, OnlineBackupError> {
        self.core
            .commit_admission()
            .begin_close()
            .map_err(map_admission)
    }

    /// Ordinary drain until settled, NO repo write permit held. A drain
    /// error is a typed fail-closed refusal (no position reported).
    async fn drain_to_settled(&self) -> Result<(), OnlineBackupError> {
        let drainer = self.core.drainer();
        let core = &self.core;
        // Box::pin: large await at the call site (Windows debug stack
        // history on this chain).
        Box::pin(drainer.drain_all(core)).await?;
        if !self.settled_under_pause() {
            return Err(OnlineBackupError::DrainUnsettled);
        }
        Ok(())
    }

    /// Pending-work check, used both after the settled drain and as the
    /// under-pause recheck: empty drain window, no unresolved WAL
    /// versions, and the durable watermark caught up to the last committed
    /// version.
    fn settled_under_pause(&self) -> bool {
        if self.core.drainer().pending_window_len() != 0 {
            return false;
        }
        match self.core.initialized_tx_gate() {
            Some(gate) => {
                gate.unresolved_wal_versions().is_empty()
                    && gate.durable_watermark() == gate.last_committed()
            }
            // An untouched gate: nothing was ever committed.
            None => true,
        }
    }
}
