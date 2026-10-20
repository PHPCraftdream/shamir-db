use std::future::Future;
use std::time::Duration;

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use shamir_tx::ChangelogEvent;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::time::Instant;

use super::journal_source::{JournalReceiver, JournalSource};
#[cfg(test)]
use crate::subscriptions::tests::bridge::bridge_snapshot_cut_tests;

/// Task-local processed-through state, not the last frame version.
pub(crate) struct JournalDrain {
    pub cursor: u64,
    hint: u64,
    dirty: bool,
    progress: Option<u64>,
    retry: Option<Instant>,
    backoff: Duration,
}

impl JournalDrain {
    pub fn new(cursor: u64) -> Self {
        Self {
            cursor,
            hint: cursor,
            dirty: true,
            progress: None,
            retry: None,
            backoff: Duration::from_millis(5),
        }
    }

    pub fn request_drain(&mut self) {
        self.dirty = true;
    }

    fn hint(&mut self, version: Option<u64>) {
        if let Some(version) = version {
            self.hint = self.hint.max(version);
        }
        // Even an older hint can signal newly settled progress.
        self.dirty = true;
    }

    /// Run the production drain; delivery futures are never raced against hints.
    /// cancel-safe: NO — cancellation is terminal (possibly accepted partial event).
    #[allow(clippy::too_many_arguments)]
    pub async fn run<S, D, F, G, GF>(
        source: &S,
        repos: &[String],
        receivers: &mut [impl JournalReceiver],
        states: &mut [Self],
        stop_when_clean: bool,
        mut deliver: D,
        mut gap: G,
    ) -> Result<(), String>
    where
        S: JournalSource,
        D: FnMut(usize, ChangelogEvent) -> F,
        F: Future<Output = bool>,
        G: FnMut(u64) -> GF,
        GF: Future<Output = ()>,
    {
        loop {
            for (ri, state) in states.iter_mut().enumerate() {
                if !state.dirty {
                    continue;
                }
                state.dirty = false;
                let Some(from) = state.cursor.checked_add(1) else {
                    state.progress = None;
                    state.retry = None;
                    continue;
                };
                let page = source.read(&repos[ri], from).await?;
                // Deterministic post-read / pre-processing interleaving for quarantine tests.
                #[cfg(test)]
                if !page.read.events.is_empty() {
                    bridge_snapshot_cut_tests::pause(3, from).await;
                }
                if source.quarantined(&repos[ri]) {
                    return Err("journal repository quarantined".to_owned());
                }
                if let Some(at) = page.read.gap_at {
                    gap(at).await;
                    return Err("journal gap requires resynchronization".to_owned());
                }
                let empty = page.read.events.is_empty();
                for event in page.read.events {
                    let version = event.commit_version;
                    if !deliver(ri, event).await {
                        return Err("required event rejected".to_owned());
                    }
                    state.cursor = version;
                }
                if empty && page.confirmed {
                    state.cursor = state.cursor.max(page.bound);
                }
                state.progress = (state.hint > page.sampled_w).then_some(state.hint);
                if !page.confirmed {
                    // A final consumed hint must not strand persistence confirmation.
                    if empty {
                        state.retry = Some(Instant::now() + state.backoff);
                        state.backoff = (state.backoff * 2).min(Duration::from_millis(250));
                    } else {
                        state.dirty = true;
                    }
                } else {
                    state.retry = None;
                    state.backoff = Duration::from_millis(5);
                    state.dirty = !empty;
                }
                // One page per repo per round; short pages still page again.
                tokio::task::yield_now().await;
            }
            // Coalesce hints before another page, without trusting their payloads.
            for (ri, receiver) in receivers.iter_mut().enumerate() {
                // Bound coalescing so a busy producer cannot monopolize this task.
                for _ in 0..64 {
                    match receiver.try_recv() {
                        Ok(event) => states[ri].hint(Some(event.commit_version)),
                        Err(TryRecvError::Lagged(_)) => states[ri].hint(None),
                        Err(TryRecvError::Closed) => return Err("changefeed closed".to_owned()),
                        Err(TryRecvError::Empty) => break,
                    }
                }
            }
            let dirty = states.iter().any(|state| state.dirty);
            let pending = states
                .iter()
                .any(|state| state.progress.is_some() || state.retry.is_some());
            if stop_when_clean && !pending && !dirty {
                return Ok(());
            }
            if !dirty && !pending {
                source.on_idle();
            }
            // At most one wait per repo, plus one receiver per repo. Cancellation
            // drops observation-only progress/timers and cancel-safe recv futures.
            let mut waits = FuturesUnordered::new();
            for (ri, state) in states.iter().enumerate() {
                if state.progress.is_some() || state.retry.is_some() {
                    let repo = &repos[ri];
                    let target = state.progress;
                    let retry = state.retry;
                    waits.push(async move {
                        let progress = async {
                            match target {
                                Some(target) => source.progress(repo, target).await,
                                None => std::future::pending().await,
                            }
                        };
                        let timer = async {
                            match retry {
                                Some(deadline) => tokio::time::sleep_until(deadline).await,
                                None => std::future::pending().await,
                            }
                        };
                        let result = tokio::select! {
                            result = progress => result,
                            () = timer => Ok(()),
                        };
                        (ri, result)
                    });
                }
            }
            let mut hints = FuturesUnordered::new();
            for (ri, receiver) in receivers.iter_mut().enumerate() {
                hints.push(async move { (ri, receiver.recv().await) });
            }
            tokio::select! {
                biased;
                Some((ri, result)) = waits.next(), if pending => {
                    result?;
                    states[ri].progress = None;
                    states[ri].retry = None;
                    states[ri].dirty = true;
                }
                _ = std::future::ready(()), if dirty => {}
                Some((ri, result)) = hints.next() => {
                    match result {
                        Ok(event) => states[ri].hint(Some(event.commit_version)),
                        Err(RecvError::Lagged(_)) => states[ri].hint(None),
                        Err(RecvError::Closed) => return Err("changefeed closed".to_owned()),
                    }
                }
            }
        }
    }
}
