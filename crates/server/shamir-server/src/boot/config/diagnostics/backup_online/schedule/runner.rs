use std::sync::Arc;
use std::time::Duration;

use shamir_types::time::local_minute_of_day;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;

use super::super::metrics::{enable_schedule, note_scheduled, ScheduledResult};
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

/// cancel-safe: NO — cancel the shared registry root and join this task so it can drain an admitted run through cleanup.
pub(super) async fn run(
    (boot, interval, run_on_start): (Instant, Duration, bool),
    window: Option<DailyWindow>,
    shutdown: CancellationToken,
    mut local_minute: impl FnMut() -> u32,
    mut start: impl FnMut() -> Result<oneshot::Receiver<bool>, StartBackupError>,
    mut note: impl FnMut(ScheduledResult),
) {
    let mut next = if run_on_start {
        Some(boot)
    } else {
        boot.checked_add(interval)
    };
    let mut in_flight: Option<oneshot::Receiver<bool>> = None;
    while let Some(deadline) = next {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            result = receive(&mut in_flight) => {
                in_flight = None;
                note(terminal(result));
            }
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
                        Ok(receiver) => {
                            in_flight = Some(receiver);
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
    if let Some(receiver) = in_flight {
        note(terminal(receiver.await));
    }
}

/// cancel-safe: yes — polling borrows the receiver; a losing branch leaves it available to drain.
async fn receive(
    in_flight: &mut Option<oneshot::Receiver<bool>>,
) -> Result<bool, oneshot::error::RecvError> {
    match in_flight {
        Some(receiver) => receiver.await,
        None => std::future::pending().await,
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
