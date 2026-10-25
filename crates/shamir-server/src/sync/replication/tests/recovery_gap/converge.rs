//! Bounded-convergence helpers for the recovery-gap tests.
//!
//! WHY this exists: `run_follower_loop` with `with_max_iterations(1)` counts
//! ATTEMPTS, not progress. An empty page (the leader clamps visibility to the
//! WAL recoverability prefix with a 200 ms bounded wait), a swallowed
//! transient error, or a structured leader error all consume the single
//! iteration and return `Ok(())` WITHOUT advancing the bookmark. Treating
//! `Ok(())` as "caught up" is therefore racy. These helpers retry bounded
//! single iterations and report the RECORDED pull outcomes on failure.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use shamir_db::engine::tx::ReplicatedEvent;
use shamir_db::ShamirDb;
use shamir_query_types::wire::repl::ReplResponse;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::ReplSourceFactory;

use super::super::supervisor_tests::sub_state;

const MAX_ATTEMPTS: usize = 50;
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
const BETWEEN_ATTEMPTS: Duration = Duration::from_millis(10);
const WAIT_STEPS: usize = 600;

/// Last-8 rolling log of recorded pull outcomes.
#[derive(Clone, Default)]
pub(crate) struct OutcomeLog(Arc<Mutex<Vec<String>>>);

impl OutcomeLog {
    fn record(&self, line: String) {
        let mut entries = self.0.lock().unwrap();
        entries.push(line);
        let overflow = entries.len().saturating_sub(8);
        entries.drain(..overflow);
    }

    pub(crate) fn describe(&self) -> String {
        let entries = self.0.lock().unwrap();
        if entries.is_empty() {
            "no pulls observed".to_string()
        } else {
            entries.join("\n  ")
        }
    }
}

struct RecordingSource {
    inner: Arc<dyn ReplSource>,
    log: OutcomeLog,
}

#[async_trait]
impl ReplSource for RecordingSource {
    async fn hello(&self, node: &str) -> Result<ReplResponse, ReplError> {
        self.inner.hello(node).await
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        let outcome = self.inner.pull(db, repo, from_version, limit, wait).await;
        match &outcome {
            Ok(response) => self.log.record(describe_ok(from_version, response)),
            Err(error) => self
                .log
                .record(format!("pull(from={from_version}) -> Err({error:?})")),
        }
        outcome
    }
}

fn describe_ok(from_version: u64, response: &ReplResponse) -> String {
    match response {
        ReplResponse::Pull {
            events,
            gap_at,
            current_version,
            ..
        } => {
            let versions: Vec<u64> = rmp_serde::from_slice::<Vec<ReplicatedEvent>>(events)
                .map(|decoded| {
                    decoded
                        .iter()
                        .map(|event| event.event.commit_version)
                        .collect()
                })
                .unwrap_or_default();
            format!(
                "pull(from={from_version}) -> events={versions:?} gap_at={gap_at:?} head={current_version}"
            )
        }
        ReplResponse::Error { code, message, .. } => {
            format!("pull(from={from_version}) -> structured error {code}: {message}")
        }
        ReplResponse::Hello { .. } => {
            format!("pull(from={from_version}) -> unexpected Hello reply")
        }
    }
}

async fn bookmark_of(db: &ShamirDb) -> u64 {
    db.get_db("app")
        .expect("follower db")
        .get_repo("main")
        .expect("follower repo")
        .replication_bookmark()
        .await
        .expect("bookmark")
}

/// Runs bounded single-iteration follower attempts until the durable
/// bookmark reaches `target`. A returned `Err` panics immediately (terminal
/// results are never retried away).
pub(crate) async fn run_until_bookmark(follower: &ShamirDb, leader: &ShamirDb, target: u64) {
    let log = OutcomeLog::default();
    for attempt in 1..=MAX_ATTEMPTS {
        let source = Arc::new(RecordingSource {
            inner: Arc::new(InProcessReplSource::new(leader.clone())),
            log: log.clone(),
        });
        let attempt_future = run_follower_loop(
            Arc::new(follower.clone()),
            source,
            FollowerLoopConfig::new("follower", "app", "main")
                .with_poll_wait_ms(0)
                .with_max_iterations(1),
            CancellationToken::new(),
        );
        match tokio::time::timeout(ATTEMPT_TIMEOUT, attempt_future).await {
            Err(_elapsed) => panic!(
                "attempt {attempt}/{MAX_ATTEMPTS} timed out waiting for bookmark {target}; pulls:\n  {}",
                log.describe()
            ),
            Ok(Err(error)) => panic!(
                "attempt {attempt}/{MAX_ATTEMPTS} returned terminal error {error:?}; pulls:\n  {}",
                log.describe()
            ),
            Ok(Ok(())) => {}
        }
        if bookmark_of(follower).await == target {
            return;
        }
        tokio::time::sleep(BETWEEN_ATTEMPTS).await;
    }
    let actual = bookmark_of(follower).await;
    panic!(
        "bookmark never reached {target} after {MAX_ATTEMPTS} attempts (last: {actual}); pulls:\n  {}",
        log.describe()
    );
}

/// A supervisor [`ReplSourceFactory`] whose every source records its pulls
/// into the SAME returned [`OutcomeLog`].
pub(crate) fn recording_factory(leader: &ShamirDb) -> (ReplSourceFactory, OutcomeLog) {
    let leader = leader.clone();
    let factory_log = OutcomeLog::default();
    let factory: ReplSourceFactory = {
        let factory_log = factory_log.clone();
        Arc::new(move |_| {
            Arc::new(RecordingSource {
                inner: Arc::new(InProcessReplSource::new(leader.clone())),
                log: factory_log.clone(),
            }) as Arc<dyn ReplSource>
        })
    };
    (factory, factory_log)
}

/// Bounded poll of the durable bookmark (for supervisor-driven loops).
pub(crate) async fn wait_for_bookmark(follower: &ShamirDb, target: u64, log: &OutcomeLog) {
    for _ in 0..WAIT_STEPS {
        if bookmark_of(follower).await == target {
            return;
        }
        tokio::time::sleep(BETWEEN_ATTEMPTS).await;
    }
    let actual = bookmark_of(follower).await;
    panic!(
        "bookmark never reached {target} (last: {actual}); pulls:\n  {}",
        log.describe()
    );
}

/// Bounded poll of the file-local subscription "sub" until it reaches the
/// expected terminal state.
pub(crate) async fn wait_for_sub_terminal_state(follower: &ShamirDb, expected: &str) {
    let mut last = None;
    for _ in 0..WAIT_STEPS {
        last = sub_state(follower, "sub").await;
        if last.as_deref() == Some(expected) {
            return;
        }
        tokio::time::sleep(BETWEEN_ATTEMPTS).await;
    }
    panic!(
        "subscription 'sub' never reached terminal state {expected:?} (last: {:?})",
        last.as_deref()
    );
}
