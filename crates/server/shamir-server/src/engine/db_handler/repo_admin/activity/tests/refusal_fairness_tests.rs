//! Per-principal fairness of the refusal-audit queue.
//!
//! The share is charged per principal NAME, so two distinct principals can
//! never share one counter: `colliding_names` returns a pair that DID share a
//! counter under the removed hash-bucket scheme, and the test asserts that one
//! of them reaching its share cannot deny the other while global slots are
//! free.
//!
//! No assertion runs before the appender is released and every accepted job is
//! joined: a panic in the middle of the fill loop would otherwise leave up to
//! 32 blocking jobs parked until their own deadline.

use std::hash::BuildHasher;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use shamir_collections::TMap;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_query_types::admin::activity::{CancelOutcome, RequestExecId};

use crate::db_handler::repo_audit::{
    enqueue_cancel_refusal_audit, CancelRefusalAudit, CANCEL_AUDIT_PRINCIPAL_SHARE,
};

const TIMEOUT: Duration = Duration::from_secs(120);

fn session(name: &str, sid: u8) -> Session {
    let mut session = Session::new(
        [sid; 16],
        name.to_owned(),
        SessionPermissions::new(false, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::None,
        [0; 32],
        1_000,
    );
    session.session_id = [sid; 32];
    session
}

/// Two DISTINCT principal names that shared ONE counter under the removed
/// hash-bucket scheme (`THasher::hash_one(name) % 32`). With exact per-name
/// accounting they can no longer deny each other, and using such a pair keeps
/// a regression to bucket accounting visible.
fn colliding_names() -> (String, String) {
    // 33 names cannot fit 32 buckets (pigeonhole), so the loop always finds a
    // collision; `REMOVED_BUCKETS` is the width the old scheme used.
    const REMOVED_BUCKETS: u64 = 32;
    let mut seen: TMap<u64, String> = TMap::default();
    for index in 0..=REMOVED_BUCKETS as u32 {
        let name = format!("principal-{index}");
        let bucket =
            shamir_collections::THasher::default().hash_one(name.as_str()) as u64 % REMOVED_BUCKETS;
        if let Some(other) = seen.get(&bucket) {
            return (other.clone(), name);
        }
        seen.insert(bucket, name);
    }
    unreachable!("33 names cannot fit 32 buckets")
}

#[derive(Default)]
struct GatedAppender {
    gate: Mutex<GateState>,
    release: Condvar,
    recorded: Mutex<Vec<AuditEntry>>,
}

#[derive(Default)]
struct GateState {
    entered: usize,
    released: bool,
}

impl GatedAppender {
    /// Unblock every parked `append_entry`, including future calls.
    fn release_all(&self) {
        self.gate.lock().unwrap().released = true;
        self.release.notify_all();
    }

    fn entered(&self) -> usize {
        self.gate.lock().unwrap().entered
    }

    fn recorded(&self) -> Vec<AuditEntry> {
        self.recorded.lock().unwrap().clone()
    }
}

impl AuditAppender for GatedAppender {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        // Synchronous by trait: this runs inside the blocking audit job, so
        // the guard never crosses an `.await`.
        let mut gate = self.gate.lock().unwrap();
        gate.entered += 1;
        self.release.notify_all();
        while !gate.released {
            let (next, waited) = self
                .release
                .wait_timeout(gate, TIMEOUT)
                .map_err(|_| std::io::Error::other("gated appender mutex poisoned"))?;
            gate = next;
            if waited.timed_out() && !gate.released {
                return Err(std::io::Error::other("gated appender release timed out"));
            }
        }
        drop(gate);
        self.recorded.lock().unwrap().push(entry.clone());
        Ok(())
    }

    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

#[tokio::test]
async fn one_principal_cannot_exceed_its_share_while_another_is_recorded() {
    let (name_a, name_b) = colliding_names();
    let appender = Arc::new(GatedAppender::default());
    let writer = Arc::new(AuditChainWriter::new_with_shared(
        Arc::new(AuditChain::new([7; 32])),
        appender.clone(),
    ));
    let queue = Arc::new(CancelRefusalAudit::default());
    let principal_a = session(&name_a, 1);
    let principal_b = session(&name_b, 2);

    // A fill loop that never asserts: the release below must be reachable on
    // every path.
    let mut accepted = Vec::with_capacity(CANCEL_AUDIT_PRINCIPAL_SHARE + 2);
    let mut admitted_a = 0;
    for target in 0..CANCEL_AUDIT_PRINCIPAL_SHARE as u64 {
        let job = enqueue_cancel_refusal_audit(
            &queue,
            &writer,
            &principal_a,
            RequestExecId(target),
            &CancelOutcome::NotCancellable,
        );
        if let Some(job) = job {
            admitted_a += 1;
            accepted.push(job);
        }
    }
    let over_share = enqueue_cancel_refusal_audit(
        &queue,
        &writer,
        &principal_a,
        RequestExecId(u64::MAX),
        &CancelOutcome::NotCancellable,
    );
    let over_share_refused = over_share.is_none();
    if let Some(job) = over_share {
        accepted.push(job);
    }
    let from_other = enqueue_cancel_refusal_audit(
        &queue,
        &writer,
        &principal_b,
        RequestExecId(u64::MAX),
        &CancelOutcome::NotCancellable,
    );
    let other_accepted = from_other.is_some();
    if let Some(job) = from_other {
        accepted.push(job);
    }
    let drops = queue.dropped_count();

    // Release unconditionally: an assertion must never be able to leave a
    // blocking append parked until its own deadline.
    appender.release_all();
    let accepted_count = accepted.len();
    for job in accepted {
        tokio::time::timeout(TIMEOUT, job)
            .await
            .expect("accepted refusal audit job timed out")
            .unwrap();
    }
    let entered = appender.entered();

    assert_eq!(
        admitted_a, CANCEL_AUDIT_PRINCIPAL_SHARE,
        "every refusal inside the principal share must be admitted"
    );
    assert!(
        over_share_refused,
        "principal A exceeded its share of the audit queue"
    );
    assert!(
        other_accepted,
        "principal B was denied by principal A's share: these two names shared one counter under \
         the removed bucket scheme, and exact per-name accounting must not"
    );
    assert_eq!(
        drops, 1,
        "exactly the over-share refusal is counted as a dropped event"
    );
    assert_eq!(accepted_count, CANCEL_AUDIT_PRINCIPAL_SHARE + 1);
    assert_eq!(entered, CANCEL_AUDIT_PRINCIPAL_SHARE + 1);

    let recorded = appender.recorded();
    assert_eq!(recorded.len(), CANCEL_AUDIT_PRINCIPAL_SHARE + 1);
    let count_for = |principal: &Session| {
        recorded
            .iter()
            .filter(|entry| entry.user == principal.username)
            .count()
    };
    assert_eq!(count_for(&principal_a), CANCEL_AUDIT_PRINCIPAL_SHARE);
    assert_eq!(count_for(&principal_b), 1);
    let recorded_principals: shamir_collections::TSet<&str> =
        recorded.iter().map(|entry| entry.user.as_str()).collect();
    assert_eq!(recorded_principals.len(), 2);
    assert!(recorded_principals.contains(principal_a.username.as_str()));
    assert!(recorded_principals.contains(principal_b.username.as_str()));

    // Every slot above was released when its job finished, so principal A is
    // admissible again. A counter that never decrements (or that keeps A's
    // share charged forever) refuses here.
    let again = enqueue_cancel_refusal_audit(
        &queue,
        &writer,
        &principal_a,
        RequestExecId(u64::MAX - 1),
        &CancelOutcome::NotCancellable,
    );
    let re_admitted = again.is_some();
    if let Some(job) = again {
        let _ = tokio::time::timeout(TIMEOUT, job).await;
    }
    assert!(
        re_admitted,
        "a finished refusal must release its share slot for the same principal"
    );
}
