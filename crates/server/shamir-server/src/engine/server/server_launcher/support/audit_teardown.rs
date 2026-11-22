//! One bounded, truthful audit teardown shared by every shutdown caller.
//!
//! The order is the guarantee: admission is closed and the scheduler is
//! signalled first, the refusal-audit queue is drained, and only then is the
//! scheduler JOINED — before the appender's flusher is joined and before its
//! final drain, anchor write and fjall `SyncAll`. A scheduled audit
//! checkpoint therefore can no longer interleave with the final log sync and
//! anchor write. That final flush acquires the same synchronous persistence
//! gate a checkpoint takes, on a blocking thread, and it takes it only after
//! the scheduler has stopped, so the last close runs under the same gate
//! discipline as a checkpoint.
//!
//! The audit stage owns the two things that can stall a stop: the
//! refusal-audit queue (accepted `spawn_blocking` append jobs) and the audit
//! appender (its background flusher plus that final drain, anchor write and
//! `SyncAll`). The scheduler join, the flusher join and the final flush are
//! all bounded by the one [`AUDIT_TEARDOWN_DEADLINE`], the blocking part runs
//! on a blocking thread instead of inside the shutdown future's own poll, and
//! every failure the appender used to discard — flusher join, final flush,
//! chain anchor, server-meta hook — reaches [`ShutdownReport`] once, at the
//! moment it is observed.
//!
//! A timeout stops waiting; it does not cancel an in-flight `fsync` on the
//! blocking thread. Teardown continues to the repository flush regardless:
//! the refusal reply and the refusal's own audit append never depend on this
//! stage.
//!
//! The one rule a deadline can break: the final close (final drain, anchor and
//! server-metadata write) is submitted only when BOTH the scheduler and the
//! flusher stopped inside the deadline. If either timed out, its task may still
//! be inside the persistence gate, so a final close submitted now could run
//! BEFORE it. In that case no close is submitted at all: the report says the
//! buffered events and the anchor were not persisted by this shutdown, and the
//! next start recovers the chain exactly as it does after a crash.

#[cfg(test)]
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use shamir_db::ShutdownReport;

use crate::audit_appender::FjallAuditAppender;
use crate::db_handler::CancelRefusalAudit;
use crate::scheduler::Scheduler;

/// One shared budget for the whole audit teardown: the refusal drain, the
/// scheduler join, the flusher join and the final flush + anchor write. 10 s
/// because the drain below may spend its own 3 s first and a loaded CI runner
/// fsyncs an order of magnitude slower than a workstation; the repository
/// flush that follows has its own budget, so this stage must never consume
/// the outer `runtime::serve` deadline on its own.
pub(crate) const AUDIT_TEARDOWN_DEADLINE: Duration = Duration::from_secs(10);

// One fixed prefix per distinct failure. `ShutdownReport` stores plain
// strings, so this prefix is the only thing an operator or a supervisor can
// grep for; each failure is reported once, at the moment it is observed, so a
// later timeout can never swallow an earlier error and no caller has to
// reinterpret a message.
const ERR_AUDIT_TIMEOUT: &str = "audit teardown exceeded";
const ERR_FLUSHER_JOIN: &str = "audit flusher task did not join cleanly";
const ERR_FLUSH: &str = "audit flush failed at shutdown";
const ERR_ANCHOR: &str = "audit anchor or metadata write failed at shutdown";
const ERR_FLUSH_TASK: &str = "audit flush task did not complete";
const ERR_SCHEDULER_STOP: &str = "scheduler did not stop within";
const ERR_FINAL_CLOSE_SKIPPED: &str = "audit final close not attempted";

/// The three audit owners a shutdown must drain, in teardown order.
pub(crate) struct AuditTeardown<'a> {
    pub(crate) cancel_refusal_audit: &'a Arc<CancelRefusalAudit>,
    pub(crate) appender: &'a Arc<FjallAuditAppender>,
    pub(crate) scheduler: Scheduler,
    /// Test seam: fires the first time the REFUSAL DRAIN itself polls
    /// `Pending`, so the refusal-drain binding test can release its gate at
    /// the exact moment teardown waits for the queue.
    #[cfg(test)]
    pub(crate) drain_probe: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Close admission, drain, join the scheduler, join the flusher and flush the
/// audit stage under one deadline.
///
/// Returns `true` when the stage completed inside the deadline and its final
/// close ran. Either way teardown is finished with the audit owners: the
/// scheduler has been told to stop and joined (or abandoned), the flusher has
/// been stopped and joined (or abandoned), and every failure has been recorded
/// in `report` once, at the moment it was observed. The final close runs only
/// after BOTH stoppers completed; if either timed out, [`ERR_FINAL_CLOSE_SKIPPED`]
/// is reported instead and nothing is submitted.
pub(crate) async fn teardown_audit(parts: AuditTeardown<'_>, report: &mut ShutdownReport) -> bool {
    let deadline = tokio::time::Instant::now() + AUDIT_TEARDOWN_DEADLINE;
    parts.cancel_refusal_audit.close();
    // Signal before joining: the scheduler has its own broadcast, not the
    // root shutdown token, so a join alone would wait for the next tick.
    parts.scheduler.signal_shutdown();

    let mut complete = true;

    // Cancel-safe: nothing is consumed by waiting. On expiry the queued jobs
    // are aborted below; the ones already inside the writer keep running.
    let refusal = parts.cancel_refusal_audit;
    #[cfg(test)]
    let drained = drain_refusal(refusal, parts.drain_probe).await;
    #[cfg(not(test))]
    let drained = drain_refusal(refusal).await;
    if !drained {
        refusal.discard_queued();
    }

    // Join the scheduler BEFORE the flusher join and the final flush: a
    // scheduled audit checkpoint checked in after `close()` could otherwise
    // interleave with the final log sync and the anchor write.
    let mut scheduler_stopped = true;
    if tokio::time::timeout_at(deadline, parts.scheduler.join())
        .await
        .is_err()
    {
        report.add_error(format!("{ERR_SCHEDULER_STOP} {AUDIT_TEARDOWN_DEADLINE:?}"));
        complete = false;
        scheduler_stopped = false;
    }

    parts.appender.request_flusher_stop();
    let mut flusher_stopped = true;
    match tokio::time::timeout_at(deadline, parts.appender.join_flusher()).await {
        // A join that returned an error still stopped the task, so the final
        // close may run; its buffered entries are drained by that close.
        Ok(Ok(())) => {}
        Ok(Err(message)) => {
            report.add_error(format!("{ERR_FLUSHER_JOIN}: {message}"));
            complete = false;
        }
        Err(_) => {
            report.add_error(format!(
                "{ERR_AUDIT_TIMEOUT} {AUDIT_TEARDOWN_DEADLINE:?} waiting for the audit flusher"
            ));
            complete = false;
            flusher_stopped = false;
        }
    }

    // ONE rule: the final close is submitted only when both stoppers finished.
    // A timed-out task can still be inside the persistence gate, so a close
    // submitted now would run before it — the ordering this stage exists for
    // would be inverted on exactly the path where it matters most.
    if !(scheduler_stopped && flusher_stopped) {
        report.add_error(format!(
            "{ERR_FINAL_CLOSE_SKIPPED}: the scheduler or the audit flusher did not stop inside \
             {AUDIT_TEARDOWN_DEADLINE:?}, so the final audit drain, the chain anchor write and the \
             server-metadata write were not started; the last buffered audit events and the anchor \
             are not persisted by this shutdown, and the next start recovers the chain exactly as \
             it does after a crash"
        ));
        return complete;
    }

    // The final drain + anchor write is synchronous (gate + fsync), so it runs
    // on a blocking thread, acquires the gate itself there and is bounded by
    // the shared deadline rather than by whoever holds the gate.
    let appender = Arc::clone(parts.appender);
    let flush = tokio::time::timeout_at(
        deadline,
        tokio::task::spawn_blocking(move || appender.final_flush_blocking()),
    )
    .await;
    match flush {
        Ok(Ok(outcome)) => {
            if let Some(message) = outcome.flush_error {
                report.add_error(format!("{ERR_FLUSH}: {message}"));
                complete = false;
            }
            if let Some(message) = outcome.anchor_error {
                report.add_error(format!("{ERR_ANCHOR}: {message}"));
                complete = false;
            }
        }
        Ok(Err(join_error)) => {
            report.add_error(format!("{ERR_FLUSH_TASK}: {join_error}"));
            complete = false;
        }
        Err(_) => {
            report.add_error(format!(
                "{ERR_AUDIT_TIMEOUT} {AUDIT_TEARDOWN_DEADLINE:?} waiting for the final audit \
                 flush — an in-flight flush is not cancelled"
            ));
            complete = false;
        }
    }

    complete
}

/// Test seam: fire `probe` the first time the REFUSAL DRAIN itself polls
/// `Pending`, so the refusal-drain binding test releases its gate at the
/// exact moment teardown waits for the queue — never for a later stage.
#[cfg(test)]
async fn drain_refusal(
    queue: &Arc<CancelRefusalAudit>,
    probe: Option<tokio::sync::oneshot::Sender<()>>,
) -> bool {
    let drain = queue.drain();
    tokio::pin!(drain);
    let mut probe = probe;
    std::future::poll_fn(move |cx| {
        let result = drain.as_mut().poll(cx);
        if result.is_pending() {
            if let Some(signal) = probe.take() {
                let _ = signal.send(());
            }
        }
        result
    })
    .await
}

/// Cancel-safe refusal drain (no test seam in production builds).
#[cfg(not(test))]
async fn drain_refusal(queue: &Arc<CancelRefusalAudit>) -> bool {
    queue.drain().await
}
