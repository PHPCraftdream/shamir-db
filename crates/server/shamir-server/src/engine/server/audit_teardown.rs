//! One bounded, truthful audit teardown shared by every shutdown caller.
//!
//! The audit stage owns the last two things that can stall a stop: the
//! refusal-audit queue (accepted `spawn_blocking` append jobs) and the audit
//! appender (its background flusher plus a final drain, anchor write and
//! fjall `SyncAll`, all under one synchronous persistence gate that a
//! checkpoint can already hold). Both are bounded here by
//! [`AUDIT_TEARDOWN_DEADLINE`], the blocking part runs on a blocking thread
//! instead of inside the shutdown future's own poll, and every failure the
//! appender used to discard — flusher join, final flush, latched anchor —
//! reaches [`ShutdownReport`].
//!
//! A timeout stops waiting; it does not cancel an in-flight `fsync` on the
//! blocking thread. Teardown continues to the repository flush regardless:
//! the refusal reply and the refusal's own audit append never depend on this
//! stage.

#[cfg(test)]
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use shamir_db::ShutdownReport;

use crate::audit_appender::FjallAuditAppender;
use crate::db_handler::CancelRefusalAudit;
use crate::scheduler::Scheduler;

/// One shared budget for the whole audit teardown: the refusal drain, the
/// flusher join and the final flush + anchor write. 10 s because the drain
/// below may spend its own 3 s first and a loaded CI runner fsyncs an order
/// of magnitude slower than a workstation; the repository flush that follows
/// has its own budget, so this stage must never consume the outer
/// `runtime::serve` deadline on its own.
pub(crate) const AUDIT_TEARDOWN_DEADLINE: Duration = Duration::from_secs(10);

/// The three audit owners a shutdown must drain, in teardown order.
pub(crate) struct AuditTeardown<'a> {
    pub(crate) cancel_refusal_audit: &'a Arc<CancelRefusalAudit>,
    pub(crate) appender: &'a Arc<FjallAuditAppender>,
    pub(crate) scheduler: Scheduler,
    /// Test seam: fires the first time the stage polls `Pending`, so the
    /// refusal-drain binding test can release its gate at the exact moment
    /// teardown is waiting for it.
    #[cfg(test)]
    pub(crate) drain_probe: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Close admission, drain, join and flush the audit stage under one deadline.
///
/// Returns `true` when the stage completed inside the deadline. Either way
/// teardown is finished with the audit owners: the scheduler has been told to
/// stop and joined (or abandoned), the appender's final flush has been
/// attempted and every failure has been recorded in `report`.
pub(crate) async fn teardown_audit(parts: AuditTeardown<'_>, report: &mut ShutdownReport) -> bool {
    let deadline = tokio::time::Instant::now() + AUDIT_TEARDOWN_DEADLINE;
    parts.cancel_refusal_audit.close();
    // Signal before joining: the scheduler has its own broadcast, not the
    // root shutdown token, so a join alone would wait for the next tick.
    parts.scheduler.signal_shutdown();

    let refusal = Arc::clone(parts.cancel_refusal_audit);
    #[cfg(test)]
    let mut drain_probe = parts.drain_probe;
    let stage = {
        let appender = Arc::clone(parts.appender);
        let refusal = Arc::clone(&refusal);
        async move {
            // Cancel-safe: nothing is consumed by waiting. On expiry the queued
            // jobs are aborted below; the ones already inside the writer keep
            // running.
            if !refusal.drain().await {
                refusal.discard_queued();
            }
            appender.request_flusher_stop();
            let flusher_join_error = appender.join_flusher().await.err();
            // The final drain + anchor write is synchronous (gate + fsync), so it
            // runs on a blocking thread and is bounded by the shared deadline
            // rather than by whoever holds the gate.
            let flush = {
                let appender = Arc::clone(&appender);
                tokio::task::spawn_blocking(move || appender.final_flush_blocking()).await
            };
            (flusher_join_error, flush)
        }
    };

    #[cfg(test)]
    let mut stage = std::pin::pin!(stage);
    #[cfg(test)]
    let stage = std::future::poll_fn(move |cx| {
        let result = stage.as_mut().poll(cx);
        if result.is_pending() {
            if let Some(signal) = drain_probe.take() {
                let _ = signal.send(());
            }
        }
        result
    });
    #[cfg(not(test))]
    let stage = stage;

    let mut complete = true;
    match tokio::time::timeout_at(deadline, stage).await {
        Ok((flusher_join_error, flush)) => {
            if let Some(error) = flusher_join_error {
                report.add_error(format!("audit flusher task did not join cleanly: {error}"));
                complete = false;
            }
            match flush {
                Ok(Ok(())) => {
                    if let Some(message) = parts.appender.last_failure() {
                        report.add_error(format!(
                            "audit anchor or metadata write failed at shutdown: {message}"
                        ));
                        complete = false;
                    }
                }
                Ok(Err(error)) => {
                    report.add_error(format!("audit flush failed at shutdown: {error}"));
                    complete = false;
                }
                Err(join_error) => {
                    report.add_error(format!("audit flush task did not complete: {join_error}"));
                    complete = false;
                }
            }
        }
        Err(_) => {
            refusal.discard_queued();
            report.add_error(format!(
                "audit teardown exceeded {AUDIT_TEARDOWN_DEADLINE:?} — \
                 an in-flight flush is not cancelled"
            ));
            complete = false;
        }
    }

    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if tokio::time::timeout(remaining, parts.scheduler.join())
        .await
        .is_err()
    {
        report.add_error(format!(
            "scheduler did not stop within {AUDIT_TEARDOWN_DEADLINE:?}"
        ));
        complete = false;
    }
    complete
}
