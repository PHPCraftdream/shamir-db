use std::cell::{Cell, RefCell};
use std::future::{poll_fn, Future};
use std::mem::discriminant;
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use shamir_private_dir::create_private_dir;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{advance, timeout, Instant};
use tokio_util::sync::CancellationToken;

use super::super::super::metrics::ScheduledResult;
use super::super::super::registry::ScheduledRunSignals;
use super::super::super::StartBackupError;
use super::super::runner::{next_deadline, run};
use crate::config::BackupScheduleConfig;

const INTERVAL: Duration = Duration::from_secs(1);
const LIMIT: Duration = Duration::from_secs(120);

type Event = (ScheduledResult, Instant);

async fn expect_event(events: &mut mpsc::Receiver<Event>, expected: ScheduledResult) -> Instant {
    let (result, at) = timeout(LIMIT, events.recv())
        .await
        .expect("scheduler outcome timeout")
        .expect("scheduler stopped before publishing outcome");
    assert_eq!(discriminant(&result), discriminant(&expected));
    at
}

#[tokio::test(start_paused = true)]
async fn first_tick_follows_run_on_start() {
    for run_on_start in [false, true] {
        let boot = Instant::now();
        let shutdown = CancellationToken::new();
        let (tx, mut events) = mpsc::channel(8);
        let task = tokio::spawn(run(
            (boot, INTERVAL, run_on_start),
            None,
            shutdown.clone(),
            || unreachable!(),
            || Err(StartBackupError::AlreadyRunning),
            move |result| tx.try_send((result, Instant::now())).unwrap(),
        ));
        let at = expect_event(&mut events, ScheduledResult::SkippedBusy).await;
        if run_on_start {
            assert_eq!(at, boot);
        } else {
            assert!((boot + INTERVAL..boot + INTERVAL * 2).contains(&at));
        }
        shutdown.cancel();
        timeout(LIMIT, task).await.unwrap().unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn daily_windows_gate_admission_at_half_open_boundaries() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let data = root.join("data");
    let destination = root.join("backups");
    std::fs::create_dir(&data).unwrap();
    create_private_dir(&destination).unwrap();
    for (window, minutes) in [
        ("08:00-10:00", [479, 480, 599, 600]),
        ("23:00-01:00", [1379, 1380, 0, 60]),
    ] {
        let shutdown = CancellationToken::new();
        let (tx, mut events) = mpsc::channel(8);
        let mut minutes = minutes.into_iter();
        let schedule = BackupScheduleConfig {
            destination: destination.clone(),
            interval: "1m".into(),
            window: Some(window.into()),
            run_on_start: true,
            retention: Default::default(),
        }
        .resolve(&data)
        .unwrap();
        let task = tokio::spawn(run(
            (Instant::now(), INTERVAL, schedule.run_on_start),
            schedule.window,
            shutdown.clone(),
            move || minutes.next().expect("unexpected extra tick"),
            || Err(StartBackupError::AlreadyRunning),
            move |result| tx.try_send((result, Instant::now())).unwrap(),
        ));
        for expected in [
            ScheduledResult::SkippedWindow,
            ScheduledResult::SkippedBusy,
            ScheduledResult::SkippedBusy,
            ScheduledResult::SkippedWindow,
        ] {
            expect_event(&mut events, expected).await;
        }
        shutdown.cancel();
        timeout(LIMIT, task).await.unwrap().unwrap();
    }
}

#[test]
fn late_tick_keeps_the_original_cadence() {
    let boot = Instant::now();
    let sampled = boot + Duration::from_millis(10_500);
    assert_eq!(
        next_deadline(boot + INTERVAL, INTERVAL, sampled),
        Some(boot + INTERVAL * 11)
    );
}

#[tokio::test(start_paused = true)]
async fn late_tick_does_not_overlap_or_replay_missed_slots() {
    let boot = Instant::now();
    let shutdown = CancellationToken::new();
    let (tx, mut events) = mpsc::channel(8);
    let (complete, outcome) = oneshot::channel();
    let (finished, finished_rx) = oneshot::channel();
    let mut first = Some(ScheduledRunSignals {
        outcome,
        finished: finished_rx,
    });
    let task = tokio::spawn(run(
        (boot, INTERVAL, true),
        None,
        shutdown.clone(),
        || unreachable!(),
        move || Ok(first.take().expect("unexpected second admission")),
        move |result| tx.try_send((result, Instant::now())).unwrap(),
    ));
    expect_event(&mut events, ScheduledResult::Started).await;
    let elapsed = Duration::from_millis(10_500);
    advance(elapsed).await;
    let late = expect_event(&mut events, ScheduledResult::SkippedBusy).await;
    assert!((boot + elapsed..boot + INTERVAL * 11).contains(&late));
    let next = expect_event(&mut events, ScheduledResult::SkippedBusy).await;
    assert!((boot + INTERVAL * 11..boot + INTERVAL * 12).contains(&next));
    complete.send(true).unwrap();
    expect_event(&mut events, ScheduledResult::Completed).await;
    // The outcome is noted but the run is not over (retention): a tick here
    // must still be busy, never a second admission.
    advance(INTERVAL).await;
    expect_event(&mut events, ScheduledResult::SkippedBusy).await;
    // The late FINISHED signal is what clears the in-flight slot.
    finished.send(()).unwrap();
    shutdown.cancel();
    timeout(LIMIT, task).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn admission_error_false_and_closed_have_distinct_accounting() {
    let shutdown = CancellationToken::new();
    let (tx, mut events) = mpsc::channel(8);
    let (failed, failed_rx) = oneshot::channel();
    failed.send(false).unwrap();
    let (failed_finished, failed_finished_rx) = oneshot::channel();
    failed_finished.send(()).unwrap();
    let (closed, closed_rx) = oneshot::channel();
    drop(closed);
    let (closed_finished, closed_finished_rx) = oneshot::channel();
    drop(closed_finished);
    let mut attempts = [
        Err(StartBackupError::InvalidDestination(String::new())),
        Ok(ScheduledRunSignals {
            outcome: failed_rx,
            finished: failed_finished_rx,
        }),
        Ok(ScheduledRunSignals {
            outcome: closed_rx,
            finished: closed_finished_rx,
        }),
    ]
    .into_iter();
    let task = tokio::spawn(run(
        (Instant::now(), INTERVAL, true),
        None,
        shutdown.clone(),
        || unreachable!(),
        move || attempts.next().expect("unexpected extra attempt"),
        move |result| tx.try_send((result, Instant::now())).unwrap(),
    ));
    for expected in [
        ScheduledResult::Failed,
        ScheduledResult::Started,
        ScheduledResult::Failed,
        ScheduledResult::Started,
        ScheduledResult::Failed,
    ] {
        expect_event(&mut events, expected).await;
    }
    shutdown.cancel();
    timeout(LIMIT, task).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_runner_token_and_drains_before_exit() {
    let shutdown = CancellationToken::new();
    let copy_token = shutdown.child_token();
    let (complete, outcome) = oneshot::channel();
    let (finished, finished_rx) = oneshot::channel();
    let mut first = Some(ScheduledRunSignals {
        outcome,
        finished: finished_rx,
    });
    let events = RefCell::new(Vec::new());
    let started = Cell::new(false);
    let boot = Instant::now();
    let mut task = pin!(run(
        (boot, INTERVAL, true),
        None,
        shutdown.clone(),
        || unreachable!(),
        || {
            assert!(!copy_token.is_cancelled());
            Ok(first.take().expect("unexpected extra admission"))
        },
        |result| {
            if matches!(result, ScheduledResult::Started) {
                started.set(true);
            }
            events.borrow_mut().push(discriminant(&result));
        },
    ));
    timeout(
        LIMIT,
        poll_fn(|cx| {
            assert!(task.as_mut().poll(cx).is_pending());
            if started.get() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }),
    )
    .await
    .unwrap();
    advance(INTERVAL).await;
    shutdown.cancel();
    assert!(copy_token.is_cancelled());
    poll_fn(|cx| {
        assert!(task.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(*events.borrow(), [discriminant(&ScheduledResult::Started)]);
    complete.send(false).unwrap();
    finished.send(()).unwrap();
    timeout(LIMIT, task).await.unwrap();
    assert_eq!(
        *events.borrow(),
        [
            discriminant(&ScheduledResult::Started),
            discriminant(&ScheduledResult::Failed)
        ],
    );
}
