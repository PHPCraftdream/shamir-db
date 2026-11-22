use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

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

fn session(user: u8, sid: u8) -> Session {
    // The share is keyed on the username, so each test principal needs a
    // distinct name; `session_id` alone would not separate the buckets.
    let mut session = Session::new(
        [user; 16],
        format!("principal-{user}"),
        SessionPermissions::new(false, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::None,
        [0; 32],
        1_000,
    );
    session.session_id = [sid; 32];
    session
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
    let appender = Arc::new(GatedAppender::default());
    let writer = Arc::new(AuditChainWriter::new_with_shared(
        Arc::new(AuditChain::new([7; 32])),
        appender.clone(),
    ));
    let queue = Arc::new(CancelRefusalAudit::default());
    let principal_a = session(1, 1);
    let principal_b = session(2, 2);

    let mut accepted = Vec::with_capacity(CANCEL_AUDIT_PRINCIPAL_SHARE + 1);
    for target in 0..CANCEL_AUDIT_PRINCIPAL_SHARE as u64 {
        let job = enqueue_cancel_refusal_audit(
            &queue,
            &writer,
            &principal_a,
            RequestExecId(target),
            &CancelOutcome::NotCancellable,
        );
        accepted.push(job.expect("refusal within the principal share must be admitted"));
    }
    let over_share = enqueue_cancel_refusal_audit(
        &queue,
        &writer,
        &principal_a,
        RequestExecId(u64::MAX),
        &CancelOutcome::NotCancellable,
    );
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

    assert!(
        over_share.is_none(),
        "principal A exceeded its share of the audit queue"
    );
    assert!(
        other_accepted,
        "principal B was blocked by principal A's share"
    );
    assert_eq!(accepted_count, CANCEL_AUDIT_PRINCIPAL_SHARE + 1);
    assert_eq!(appender.entered(), CANCEL_AUDIT_PRINCIPAL_SHARE + 1);

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
    let recorded_principals: shamir_collections::TFxSet<&str> =
        recorded.iter().map(|entry| entry.user.as_str()).collect();
    assert_eq!(recorded_principals.len(), 2);
    assert!(recorded_principals.contains(principal_a.username.as_str()));
    assert!(recorded_principals.contains(principal_b.username.as_str()));
}
