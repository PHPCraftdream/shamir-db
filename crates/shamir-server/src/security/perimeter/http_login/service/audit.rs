//! Bounded audit-event queue for HTTP-login outcomes.

use super::super::metrics::LoginResult;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{Instrument, Span};

const CAPACITY: usize = 1_024;
const MAX_EVENT_BYTES: usize = 1_024;
const SHUTDOWN_DRAIN: Duration = Duration::from_secs(2);

/// Privacy-safe bounded login audit event.
#[derive(Clone)]
pub struct LoginAudit {
    sender: mpsc::Sender<AuditEvent>,
    failures: Arc<[AtomicU64; 3]>,
}

/// Fixed event classes; payloads contain only the issued token id when relevant.
pub enum AuditEvent {
    /// Successfully issued token, identified only by its 16-byte id.
    Issued([u8; 16]),
    /// Aggregate failures for one minute, grouped by fixed result class.
    Failures {
        /// Minute associated with the aggregate.
        minute: u64,
        /// Authentication failures.
        authentication: u32,
        /// Policy failures.
        policy: u32,
        /// Invalid-input failures.
        invalid: u32,
    },
    /// A token that could not be revoked after issuance compensation, identified only by id.
    CompensationFailed([u8; 16]),
}

impl LoginAudit {
    /// Creates the bounded queue and its single listener-owned consumer task.
    pub fn start(
        shutdown: tokio_util::sync::CancellationToken,
        writer: Arc<shamir_connect::server::audit_chain::AuditChainWriter>,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let (sender, mut receiver) = mpsc::channel(CAPACITY);
        let failures = Arc::new([AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)]);
        let worker_failures = Arc::clone(&failures);
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval_at(Instant::now() + Duration::from_secs(60), Duration::from_secs(60));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut deadline = None;
            loop {
                let event = if let Some(until) = deadline {
                    if Instant::now() >= until { break; }
                    match receiver.try_recv() { Ok(event) => Some(event), Err(_) => break }
                } else {
                    tokio::select! { biased;
                        _ = shutdown.cancelled() => { receiver.close(); deadline = Some(Instant::now() + SHUTDOWN_DRAIN); None }
                        _ = interval.tick() => { flush_failures(&worker_failures, &writer).await; None }
                        event = receiver.recv() => event,
                    }
                };
                let Some(event) = event else {
                    if deadline.is_none() && receiver.is_closed() { break; }
                    continue;
                };
                if deadline.is_some_and(|until| Instant::now() >= until) {
                    metrics::counter!("shamir_login_http_audit_dropped").increment(1);
                    break;
                }
                let (name, details) = match event {
                    AuditEvent::Issued(id) => match rmp_serde::to_vec(&serde_bytes::Bytes::new(&id)) {
                        Ok(details) => ("http_login_issued", details),
                        Err(_) => { metrics::counter!("shamir_login_http_audit_dropped").increment(1); continue; }
                    },
                    AuditEvent::Failures { minute, authentication, policy, invalid } => {
                        match rmp_serde::to_vec(&(minute, authentication, policy, invalid)) {
                            Ok(details) => ("http_login_failures", details),
                            Err(_) => { metrics::counter!("shamir_login_http_audit_dropped").increment(1); continue; }
                        }
                    }
                    AuditEvent::CompensationFailed(id) => match rmp_serde::to_vec(&serde_bytes::Bytes::new(&id)) {
                        Ok(details) => ("http_login_compensation_failed", details),
                        Err(_) => { metrics::counter!("shamir_login_http_audit_dropped").increment(1); continue; }
                    },
                };
                let Some(now) = unix_now() else { metrics::counter!("shamir_login_http_audit_dropped").increment(1); continue; };
                if details.len() > MAX_EVENT_BYTES { metrics::counter!("shamir_login_http_audit_dropped").increment(1); continue; }
                if deadline.is_some_and(|until| Instant::now() >= until) { metrics::counter!("shamir_login_http_audit_dropped").increment(1); break; }
                let writer = Arc::clone(&writer);
                let span = Span::current();
                let result = tokio::task::spawn_blocking(move || { let _entered = span.enter(); writer.append(name, "token", "", "", [0; 8], "ok", details, now) }).await;
                if !matches!(result, Ok(Ok(_))) { tracing::warn!("HTTP login audit append failed"); }
            }
            if deadline.is_some_and(|until| Instant::now() >= until) {
                let authentication = worker_failures[0].swap(0, Ordering::AcqRel);
                let policy = worker_failures[1].swap(0, Ordering::AcqRel);
                let invalid = worker_failures[2].swap(0, Ordering::AcqRel);
                if authentication + policy + invalid > 0 { metrics::counter!("shamir_login_http_audit_dropped").increment(1); }
            } else {
                flush_failures(&worker_failures, &writer).await;
            }
            if receiver.len() > 0 { metrics::counter!("shamir_login_http_audit_dropped").increment(receiver.len() as u64); }
        }.instrument(Span::current()));
        (Self { sender, failures }, task)
    }

    /// Enqueue a privacy-safe token-id event without waiting for capacity.
    pub fn issued(&self, id: [u8; 16]) {
        if self.sender.try_send(AuditEvent::Issued(id)).is_err() {
            metrics::counter!("shamir_login_http_audit_dropped").increment(1);
        }
    }

    /// Count a failure in one of the three fixed aggregate classes.
    pub(crate) fn failure(&self, result: LoginResult) {
        let index = match result {
            LoginResult::AuthenticationFailed
            | LoginResult::RateLimited
            | LoginResult::ServerBusy => Some(0),
            LoginResult::PolicyDenied => Some(1),
            LoginResult::InvalidRequest => Some(2),
            LoginResult::Challenge | LoginResult::Issued => None,
        };
        if let Some(index) = index {
            self.failures[index].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a failed compensation without retaining identity or token material.
    pub(crate) fn compensation_failed(&self, token_id: [u8; 16]) {
        if self
            .sender
            .try_send(AuditEvent::CompensationFailed(token_id))
            .is_err()
        {
            metrics::counter!("shamir_login_http_audit_dropped").increment(1);
        }
    }

    /// Enqueue one minute's aggregate failure counters without waiting.
    pub fn failures(&self, minute: u64, authentication: u32, policy: u32, invalid: u32) {
        if self
            .sender
            .try_send(AuditEvent::Failures {
                minute,
                authentication,
                policy,
                invalid,
            })
            .is_err()
        {
            metrics::counter!("shamir_login_http_audit_dropped").increment(1);
        }
    }
}

async fn flush_failures(
    counts: &Arc<[AtomicU64; 3]>,
    writer: &Arc<shamir_connect::server::audit_chain::AuditChainWriter>,
) {
    let authentication = counts[0].swap(0, Ordering::AcqRel);
    let policy = counts[1].swap(0, Ordering::AcqRel);
    let invalid = counts[2].swap(0, Ordering::AcqRel);
    if authentication == 0 && policy == 0 && invalid == 0 {
        return;
    }
    let Some(now) = unix_now() else {
        metrics::counter!("shamir_login_http_audit_dropped").increment(1);
        return;
    };
    let details = match rmp_serde::to_vec(&(
        now / 60_000_000_000,
        authentication.min(u32::MAX as u64) as u32,
        policy.min(u32::MAX as u64) as u32,
        invalid.min(u32::MAX as u64) as u32,
    )) {
        Ok(details) => details,
        Err(_) => {
            metrics::counter!("shamir_login_http_audit_dropped").increment(1);
            return;
        }
    };
    let writer = Arc::clone(writer);
    let span = Span::current();
    let result = tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        writer.append(
            "http_login_failures",
            "token",
            "",
            "",
            [0; 8],
            "ok",
            details,
            now,
        )
    })
    .await;
    if !matches!(result, Ok(Ok(_))) {
        tracing::warn!("HTTP login aggregate audit append failed");
    }
}

fn unix_now() -> Option<u64> {
    shamir_connect::common::time::UnixNanos::try_now()
        .map(|value| value.as_u64())
        .ok()
}
