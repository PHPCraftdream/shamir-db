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
//! * [`RepoInstance::seal_point`] (step E): settle the drain, acquire the
//!   drain pause, recheck pending work, flush history and sample `V*`.
//!   The returned [`SealHold`] owns the pause and closed admission gate
//!   through re-copy. Dropping it releases the pause before reopening
//!   admission. [`RepoInstance::seal_point_controlled`] is the same
//!   barrier with cooperative caller cancellation/deadline observed at
//!   step boundaries only (R2a follow-up 3) — the server adapter must
//!   use it instead of dropping the future under `select`/`timeout`,
//!   because `drain_all` can be parked on the drainer's marker mutex or
//!   dispatched `spawn_blocking` work and dropping it there abandons
//!   that state.
//!
//! Position source (verified, R2a-1d): `RepoTxGate::last_committed`
//! (`shamir-tx/src/gate/repo_tx_gate/core.rs:54`) is the max
//! assigned-and-visible commit version; after gate close + zero leases +
//! settled drain it equals `V*` (the durable watermark is asserted equal
//! as part of the settle/recheck check). `pub` only as far as the R2a-3
//! adapter provably needs.

use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(any(test, feature = "test-util"))]
use std::sync::OnceLock;

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

/// Typed barrier failures. The close owner (RAII) reopens the gate and
/// the pause guard releases the drain on every error path, timeout,
/// cancellation or unwind — the guaranteed release. A FAILED barrier that
/// had already reached `flush_all_history` does NOT reverse that flush:
/// flushed history stays, but no position is reported and the caller must
/// treat the operation as refused.
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
    /// The caller-supplied cancellation closure of
    /// [`RepoInstance::seal_point_controlled`] returned `true` at a step
    /// boundary. Checked ONLY between barrier steps — never inside a
    /// mutating await (drain, flush, gate close) — so this refusal leaves
    /// the same fail-closed state as any other error path: pause released,
    /// gate reopened, no position reported.
    #[error("the seal barrier was cancelled by the caller at a step boundary")]
    Cancelled,
    /// The caller-supplied deadline of
    /// [`RepoInstance::seal_point_controlled`] expired at a step boundary
    /// (or capped the zero-lease wait). Same fail-closed release as
    /// [`OnlineBackupError::Cancelled`].
    #[error("the seal barrier exceeded the caller deadline")]
    DeadlineExceeded,
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

/// Owns the drain pause AND the gate close owner from `seal_point` until
/// drop. `Send + 'static`, so the R2a-3 server adapter can hold it across
/// the seal re-copy. While alive: the drainer applies nothing (copied
/// store files provably contain nothing above `V*`, design §2 (iii)) and
/// the gate stays CLOSED until the hold drops (reopen LAST). Normal
/// leaders are not refused — they park on the closed gate and complete
/// once it reopens; follower apply is REFUSED for as long as the hold is
/// alive. `Drop` order (struct field order): pause released FIRST, gate
/// reopened LAST, then the writer-block window duration is recorded into
/// the private per-phase cells (read via
/// [`RepoInstance::online_backup_writer_block_seconds`]). The re-copy duration cap policy lives with the caller; the hold only
/// reports how long it has been held.
pub struct SealHold {
    /// The sampled consistency position `V*`.
    position: u64,
    /// Monotonic instant the hold was CREATED (cap policy input) — kept
    /// distinct from the measured closed-window phase time.
    since: Instant,
    /// The pause guard; dropped FIRST (forward field order).
    _pause: DrainPauseGuard,
    /// The measured close owner; dropped LAST — reopens the gate and
    /// emits the close→reopen writer-block window (approximated: sampled
    /// just before the gate-close RMW).
    _close: MeasuredCloseOwner,
}

impl SealHold {
    /// The sealed consistency position (`V*`, inclusive).
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Monotonic instant the hold was CREATED (`now - this`; the cap
    /// policy lives with the caller). Deliberately NOT the writer-block
    /// window: the closed-window phase time is recorded in the private
    /// per-phase cells only.
    pub fn held_since(&self) -> Instant {
        self.since
    }
}

/// Test seam for the later cancellation tests (R2a-1e/1f): a hook fired at
/// each named barrier step so a test can drop the capture/seal future
/// exactly between steps. `test-util`-gated like the engine's other
/// cross-crate seams (`arm_sync_wal_fault`); production never sees it.
#[cfg(any(test, feature = "test-util"))]
static ONLINE_BACKUP_STEP_HOOK: OnceLock<fn(&'static str)> = OnceLock::new();

/// Last recorded closed-window duration per phase, in nanoseconds.
/// Single-store publication: the writer stores with `Release` exactly
/// once per completed window (after the gate has reopened); readers load
/// with `Acquire`. Sentinel [`u64::MAX`] = no window recorded yet; a real
/// value is always clamped strictly below the sentinel.
/// Process-local by design (§2 (iv): a crashed process releases all
/// state); deliberate plain statics, not a callback — nothing runs inside
/// a `Drop` except the reopen itself.
static CAPTURE_CLOSE_NANOS: AtomicU64 = AtomicU64::new(u64::MAX);
static SEAL_CLOSE_NANOS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Record one completed closed-window duration into the phase cell.
/// The value is clamped strictly below the [`u64::MAX`] sentinel — also
/// when the measured duration is exactly `u64::MAX` nanos (theoretical).
fn record_close_nanos(phase_cell: &AtomicU64, window: Duration) {
    let nanos = u64::try_from(window.as_nanos())
        .unwrap_or(u64::MAX)
        .min(u64::MAX - 1);
    phase_cell.store(nanos, Ordering::Release);
}

/// RAII window recorder, the SECOND field of [`MeasuredCloseOwner`]. Its
/// `Drop` runs strictly after the close owner (first field) has reopened
/// the gate, so the recorded elapsed time always covers close→reopen.
/// The start is sampled just BEFORE the gate-close RMW, so the value
/// slightly overshoots the writer-block window by that RMW — an
/// approximation, not an exact bound.
struct CloseWindow {
    phase_cell: &'static AtomicU64,
    started: Instant,
}

impl Drop for CloseWindow {
    fn drop(&mut self) {
        record_close_nanos(self.phase_cell, self.started.elapsed());
    }
}

/// Private measured close-owner wrapper: owns the gate close right and
/// records the closed window (start sampled immediately before
/// `begin_close`) at drop. Forward field drop does the ordering: `owner`
/// (the synchronous gate reopen) drops FIRST, `_window` records the
/// elapsed window LAST — no `Option`, no `expect`, no manual `Drop` on
/// this type. Moved WHOLESALE into the [`SealHold`] on the seal success
/// path, so exactly one record fires per window on success, error,
/// timeout, cancellation, panic unwind or plain hold drop.
struct MeasuredCloseOwner {
    owner: CommitAdmissionCloseOwner,
    _window: CloseWindow,
}

impl MeasuredCloseOwner {
    async fn wait_zero(&self, cap: Duration) -> Result<(), OnlineBackupError> {
        self.owner.wait_zero(cap).await.map_err(map_admission)
    }
}

/// One cooperative-control checkpoint: the controlled seal observes the
/// caller's cancellation closure and deadline ONLY at named step
/// boundaries — never by dropping a mutating await. Waiting for the
/// drainer's marker mutex is NOT the corruption point; the hazard is
/// dropping it ONCE the mutex is held and the dispatched marker write is
/// in flight, which abandons that state and can allow a late writer to
/// overwrite past the in-progress write. A fired checkpoint returns the
/// typed refusal; the RAII owners then release exactly as on any error
/// path.
fn check_control(
    deadline: Option<Instant>,
    cancelled: &(impl Fn() -> bool + Send + Sync),
) -> Result<(), OnlineBackupError> {
    if cancelled() {
        return Err(OnlineBackupError::Cancelled);
    }
    if deadline.is_some_and(|d| Instant::now() >= d) {
        return Err(OnlineBackupError::DeadlineExceeded);
    }
    Ok(())
}

/// The zero-lease wait cap for the controlled seal: the standing typed
/// cap, clamped down to the caller deadline's remainder (never up).
fn controlled_zero_cap(deadline: Option<Instant>) -> Duration {
    match deadline {
        None => BARRIER_ZERO_LEASE_CAP,
        Some(d) => BARRIER_ZERO_LEASE_CAP.min(d.saturating_duration_since(Instant::now())),
    }
}

impl RepoInstance {
    /// Install the barrier step hook (test-only; first installer wins).
    /// Associated with the publicly reachable `RepoInstance` so the seam
    /// stays live for shamir-db / e2e dev-dependency tests.
    #[cfg(any(test, feature = "test-util"))]
    #[doc(hidden)]
    pub fn set_online_backup_step_hook(hook: fn(&'static str)) {
        let _ = ONLINE_BACKUP_STEP_HOOK.set(hook);
    }

    /// Fires the step hook only under test; a no-op otherwise.
    #[inline]
    fn fire_step(step: &'static str) {
        #[cfg(any(test, feature = "test-util"))]
        {
            if let Some(hook) = ONLINE_BACKUP_STEP_HOOK.get() {
                hook(step);
            }
        }
        #[cfg(not(any(test, feature = "test-util")))]
        {
            let _ = step;
        }
    }

    /// Read the last recorded closed-window (writer-block) duration per
    /// barrier phase: `(capture, seal)`. `None` = no completed window of
    /// that phase has been recorded yet in this process. Values are the
    /// LAST completed windows (not aggregates): each completed capture or
    /// seal barrier — success, error, timeout, cancellation or unwind —
    /// overwrites its phase cell exactly once, AFTER the gate has
    /// reopened, with the monotonic close→reopen duration — sampled just
    /// before the gate-close RMW, so it approximates (slightly overshoots)
    /// the true writer-block window (design
    /// §2 (iii) order; R3 measurement input). Process-local (§2 (iv));
    /// phases of different repositories overwrite the same cells. Solely
    /// for the server's metrics scrape; no production behavior depends
    /// on it.
    pub fn online_backup_writer_block_seconds() -> (Option<f64>, Option<f64>) {
        fn read(cell: &AtomicU64) -> Option<f64> {
            let nanos = cell.load(Ordering::Acquire);
            (nanos != u64::MAX).then(|| Duration::from_nanos(nanos).as_secs_f64())
        }
        (read(&CAPTURE_CLOSE_NANOS), read(&SEAL_CLOSE_NANOS))
    }

    /// Step B flush barrier: close admission, quiesce leases, drain to
    /// settled, flush history, report the consistency position. Does NOT
    /// acquire a truncation pin (pins are held from step A by the caller,
    /// via [`RepoInstance::acquire_truncation_pin`]).
    ///
    /// Cancel-safety: NOT cancel-safe under direct dropping. The close
    /// owner (RAII) is created before the first suspension point, so a
    /// drop always reopens the gate (release, not rollback) — but this
    /// future must still be driven to completion. Waiting for the
    /// drainer's marker mutex is NOT the corruption point; the hazard is
    /// dropping it ONCE the mutex is held and the dispatched marker
    /// write is in flight: that abandons the marker state and can let a
    /// late writer overwrite past the in-progress write, breaking
    /// marker consistency. Capture has no controlled substitute —
    /// [`RepoInstance::seal_point_controlled`] is not one (seal is not
    /// capture) — so callers must simply await this to its return.
    pub async fn capture_point(&self) -> Result<u64, OnlineBackupError> {
        if self.core.drainer().pause_held() {
            return Err(OnlineBackupError::HoldActive);
        }
        // Lock order: gate close FIRST (the pause is never taken here).
        // The body below only borrows the wrapper, so it can be dropped
        // (or moved into a hold) after the body completes.
        let owner = self.barrier_owner(&CAPTURE_CLOSE_NANOS)?;
        let outcome: Result<u64, OnlineBackupError> = Box::pin(async {
            Self::fire_step("after_close");
            owner.wait_zero(BARRIER_ZERO_LEASE_CAP).await?;
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
        drop(owner); // fail-closed reopen + window record in every outcome
        outcome
    }

    /// Step E seal barrier: the design §2 (iii) order up to the hold —
    /// close gate → zero leases → settled drain (no permit held) → pause
    /// guard → non-empty-recheck fails closed → flush history → sample
    /// `V*` → hand the measured close owner WHOLESALE to the returned
    /// [`SealHold`]. Capture and seal are mutually exclusive with each
    /// other and with any live hold (typed busy errors). While the hold
    /// is alive the gate stays CLOSED (§2 (iii): reopen LAST) — leaders
    /// park, follower apply is refused; the hold's `Drop` releases the
    /// pause FIRST and reopens the gate LAST, recording the close→reopen
    /// writer-block window (approximated around the gate-close RMW).
    ///
    /// Cancel-safety: NOT cancel-safe under direct dropping — same
    /// release-not-rollback as [`RepoInstance::capture_point`] (gate is
    /// always reopened, pause released, flushed history kept), but
    /// dropping it ONCE the drain holds the marker mutex and its
    /// dispatched write is in flight abandons that state and can allow a
    /// late overwrite past the in-progress write.
    /// Never wrap in `select`/`timeout`; use
    /// [`RepoInstance::seal_point_controlled`] for cancellation. Drop
    /// order on ANY exit past the pause: pause released FIRST, gate
    /// reopened LAST (struct field order on success; forward drop of the
    /// async block's locals then the outer owner otherwise) — admission
    /// is never reopened while the drain pause is still held.
    pub async fn seal_point(&self) -> Result<SealHold, OnlineBackupError> {
        self.seal_point_controlled(None, || false).await
    }

    /// Cooperatively controlled variant of [`RepoInstance::seal_point`]
    /// (R2a follow-up 3): the SAME barrier, observed by the caller's
    /// `cancelled` closure and `deadline` at named step boundaries only —
    /// before the gate close, at each existing barrier step seam
    /// (`after_close`, `after_quiesce`, `after_drain`, `after_pause`,
    /// `after_recheck`) and after the flush. NEVER cancels a mutating
    /// await: `drain_all`, the pause acquisition, `flush_all_history` and
    /// the gate-close RMW always run to their natural completion; the
    /// closure is consulted strictly between them. On a fired checkpoint
    /// the typed [`OnlineBackupError::Cancelled`] /
    /// [`OnlineBackupError::DeadlineExceeded`] is returned and the RAII
    /// owners fail closed exactly as on any error path (pause released
    /// first, gate reopened last, flushed history not reversed).
    ///
    /// The zero-lease wait keeps its standing typed cap
    /// (`BARRIER_ZERO_LEASE_CAP`, mapped to
    /// [`OnlineBackupError::QuiesceTimeout`]); a nearer `deadline` only
    /// ever clamps that cap DOWN to the remaining time, so the wait can
    /// also end in `DeadlineExceeded`.
    ///
    /// `deadline = None` and an always-false closure reproduce
    /// [`RepoInstance::seal_point`] exactly.
    ///
    /// Cancel-safety: safe ONLY via its cooperative signals. This future
    /// must be driven until it returns; in exchange the caller never
    /// needs to drop it mid-await — cancellation/deadline take effect at
    /// step boundaries only, so the marker mutex is never abandoned
    /// while its dispatched write is in flight.
    pub async fn seal_point_controlled(
        &self,
        deadline: Option<Instant>,
        cancelled: impl Fn() -> bool + Send + Sync,
    ) -> Result<SealHold, OnlineBackupError> {
        // Checkpoint BEFORE the gate close: a refused call never takes the
        // close CAS at all.
        check_control(deadline, &cancelled)?;
        if self.core.drainer().pause_held() {
            return Err(OnlineBackupError::HoldActive);
        }
        // Lock order: gate close FIRST, pause SECOND (below). The body
        // only borrows the wrapper so it can be moved into the hold.
        let owner = self.barrier_owner(&SEAL_CLOSE_NANOS)?;
        let outcome: Result<(u64, DrainPauseGuard), OnlineBackupError> = Box::pin(async {
            Self::fire_step("after_close");
            // Checkpoint before the zero-lease wait: a cancel that fires
            // at this seam must never even begin the quiesce wait.
            check_control(deadline, &cancelled)?;
            let zero_cap = controlled_zero_cap(deadline);
            let quiesced = owner.wait_zero(zero_cap).await;
            check_control(deadline, &cancelled)?;
            quiesced?;
            Self::fire_step("after_quiesce");
            check_control(deadline, &cancelled)?;
            // The last drain_all pass held `replaying`, so any in-flight
            // background pass finished before it returned; draining BEFORE
            // pausing avoids the recursive `replaying` acquisition
            // (hazard 1).
            self.drain_to_settled().await?;
            Self::fire_step("after_drain");
            check_control(deadline, &cancelled)?;
            let guard = self
                .core
                .drainer()
                .pause()
                .acquire()
                .await
                .map_err(|_| OnlineBackupError::HoldActive)?;
            Self::fire_step("after_pause");
            check_control(deadline, &cancelled)?;
            // Recheck under the pause: nothing may be pending. Non-empty
            // fails closed; the guard drops on the error path, releasing
            // the pause.
            if !self.settled_under_pause() {
                return Err(OnlineBackupError::RecheckPending);
            }
            Self::fire_step("after_recheck");
            check_control(deadline, &cancelled)?;
            self.core.flush_all_history().await?;
            Self::fire_step("after_flush");
            check_control(deadline, &cancelled)?;
            let position = self
                .core
                .initialized_tx_gate()
                .map(|gate| gate.last_committed())
                .unwrap_or(0);
            Ok((position, guard))
        })
        .await;
        outcome.map(|(position, guard)| {
            // Move the wrapper WHOLESALE (same `owner`/`started`/`phase`)
            // into the hold; it keeps the gate closed and records the
            // window at the hold's drop.
            SealHold {
                position,
                since: Instant::now(),
                // Field order IS the drop order (§B4: struct fields drop
                // forward): pause released FIRST, gate reopened LAST.
                _pause: guard,
                _close: owner,
            }
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

    /// Busy probe + close: the single-owner CAS decides; the close owner
    /// is created synchronously (before any suspension point) so a
    /// cancelled barrier can never leak a closed gate. The window start
    /// is sampled immediately BEFORE `begin_close`, so the recorded
    /// window always includes the full close→reopen span, and is handed
    /// to the wrapper as the second field (dropped after the owner).
    fn barrier_owner(
        &self,
        phase_cell: &'static AtomicU64,
    ) -> Result<MeasuredCloseOwner, OnlineBackupError> {
        let started = Instant::now();
        let owner = self
            .core
            .commit_admission()
            .begin_close()
            .map_err(map_admission)?;
        Ok(MeasuredCloseOwner {
            owner,
            _window: CloseWindow {
                phase_cell,
                started,
            },
        })
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
