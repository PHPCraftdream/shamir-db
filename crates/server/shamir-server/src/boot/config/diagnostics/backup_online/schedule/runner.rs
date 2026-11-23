use std::sync::Arc;
use std::time::Duration;

use shamir_types::time::local_minute_of_day;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;

use super::super::metrics::{enable_schedule, note_scheduled, ScheduledResult};
use super::super::registry::ScheduledRunSignals;
use super::super::{OnlineBackupRegistry, StartBackupError};
use crate::config::{DailyWindow, ResolvedSchedule};

pub(crate) fn spawn(
    schedule: ResolvedSchedule,
    registry: Arc<OnlineBackupRegistry>,
    owner: String,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    spawn_with_observer(schedule, registry, owner, shutdown, |_| {})
}

#[cfg(test)]
pub(crate) fn spawn_for_test(
    mut schedule: ResolvedSchedule,
    registry: Arc<OnlineBackupRegistry>,
    owner: String,
    shutdown: CancellationToken,
    interval: Duration,
    on_terminal: impl FnMut(bool) + Send + 'static,
) -> JoinHandle<()> {
    schedule.interval = interval;
    spawn_with_observer(schedule, registry, owner, shutdown, on_terminal)
}

fn spawn_with_observer(
    schedule: ResolvedSchedule,
    registry: Arc<OnlineBackupRegistry>,
    owner: String,
    shutdown: CancellationToken,
    mut on_terminal: impl FnMut(bool) + Send + 'static,
) -> JoinHandle<()> {
    let boot = Instant::now();
    enable_schedule();
    tokio::spawn(run(
        (boot, schedule.interval, schedule.run_on_start),
        schedule.window,
        shutdown,
        local_minute_of_day,
        move || {
            registry.start_scheduled(
                schedule.destination.clone(),
                owner.clone(),
                schedule.retention,
            )
        },
        move |result| {
            note_scheduled(result);
            match result {
                ScheduledResult::Completed => on_terminal(true),
                ScheduledResult::Failed => on_terminal(false),
                _ => {}
            }
        },
    ))
}

/// One admitted run the scheduler is still tracking. The slot clears only on
/// the LATE `finished` signal, so a tick during retention is still busy.
struct InFlight {
    outcome: oneshot::Receiver<bool>,
    finished: oneshot::Receiver<()>,
    /// The COPY outcome was already noted; only `finished` is still awaited.
    outcome_received: bool,
}

/// Which of a run's two signals arrived first.
enum Signal {
    Outcome(Result<bool, oneshot::error::RecvError>),
    Finished,
}

/// cancel-safe: NO — cancel the shared registry root and join this task so it can drain an admitted run through cleanup.
pub(super) async fn run(
    (boot, interval, run_on_start): (Instant, Duration, bool),
    window: Option<DailyWindow>,
    shutdown: CancellationToken,
    mut local_minute: impl FnMut() -> u32,
    mut start: impl FnMut() -> Result<ScheduledRunSignals, StartBackupError>,
    mut note: impl FnMut(ScheduledResult),
) {
    let mut next = if run_on_start {
        Some(boot)
    } else {
        boot.checked_add(interval)
    };
    let mut in_flight: Option<InFlight> = None;
    while let Some(deadline) = next {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            signal = advance(&mut in_flight) => match signal {
                Signal::Outcome(result) => {
                    if let Some(run) = in_flight.as_mut() {
                        run.outcome_received = true;
                    }
                    note(terminal(result));
                }
                Signal::Finished => in_flight = None,
            },
            () = async {
                // Already-due ticks must not wait for Tokio's timer granularity.
                if deadline > Instant::now() {
                    sleep_until(deadline).await;
                }
            } => {
                if shutdown.is_cancelled() {
                    break;
                }
                if window.as_ref().is_some_and(|window| !window.contains(local_minute())) {
                    note(ScheduledResult::SkippedWindow);
                } else if in_flight.is_some() {
                    note(ScheduledResult::SkippedBusy);
                } else {
                    match start() {
                        Ok(signals) => {
                            in_flight = Some(InFlight {
                                outcome: signals.outcome,
                                finished: signals.finished,
                                outcome_received: false,
                            });
                            note(ScheduledResult::Started);
                        }
                        Err(StartBackupError::AlreadyRunning) => note(ScheduledResult::SkippedBusy),
                        Err(_) => note(ScheduledResult::Failed),
                    }
                }
                next = next_deadline(deadline, interval, Instant::now());
            }
        }
    }
    if next.is_none() {
        tracing::warn!("scheduled backup stopped: next deadline is not representable");
    }
    if let Some(run) = in_flight {
        // Drain the admitted run: exactly one outcome note, then the late
        // signal once retention and the final publication are over.
        if !run.outcome_received {
            note(terminal(run.outcome.await));
        }
        let _ = run.finished.await;
    }
}

/// cancel-safe: yes — polling borrows the run's receivers; a losing branch
/// leaves them available to drain.
async fn advance(in_flight: &mut Option<InFlight>) -> Signal {
    let Some(run) = in_flight.as_mut() else {
        return std::future::pending().await;
    };
    let outcome_received = run.outcome_received;
    let outcome = &mut run.outcome;
    let finished = &mut run.finished;
    // Precedence keeps one note per run: a FINISHED that races ahead of the
    // outcome (the unwind path) waits until the outcome has been noted.
    tokio::select! {
        result = async {
            if outcome_received {
                std::future::pending::<Result<bool, oneshot::error::RecvError>>().await
            } else {
                outcome.await
            }
        } => Signal::Outcome(result),
        result = async {
            if outcome_received {
                finished.await
            } else {
                std::future::pending::<Result<(), oneshot::error::RecvError>>().await
            }
        } => {
            let _ = result;
            Signal::Finished
        },
    }
}

fn terminal(result: Result<bool, oneshot::error::RecvError>) -> ScheduledResult {
    if matches!(result, Ok(true)) {
        ScheduledResult::Completed
    } else {
        ScheduledResult::Failed
    }
}

pub(super) fn next_deadline(
    deadline: Instant,
    interval: Duration,
    now: Instant,
) -> Option<Instant> {
    const NANOS_PER_SECOND: u128 = 1_000_000_000;
    let period = interval.as_nanos();
    let elapsed = now.saturating_duration_since(deadline).as_nanos();
    let delay = period - elapsed.checked_rem(period)?;
    let secs = u64::try_from(delay / NANOS_PER_SECOND).ok()?;
    let nanos = u32::try_from(delay % NANOS_PER_SECOND).ok()?;
    now.checked_add(Duration::new(secs, nanos))
}
