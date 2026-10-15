use shamir_collections::TFxMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::Notify;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    HistoryWritten,
    AfterPerEventHistoryWrites,
    JournalConfirmed,
    AfterJournalFlush,
    BeforeCombinedHistoryFlush,
    BookmarkPersisted,
    ReceiptPrepared,
    ReceiptMaterialized,
    ReceiptCheckpointed,
}

#[derive(Default)]
pub(crate) struct Hook {
    pub fail_journal_barrier: AtomicBool,
    pub bypass_journal_barrier: AtomicBool,
    pub fail_history_flush: AtomicBool,
    pub park_at: Mutex<Option<Phase>>,
    pub abort_at: Mutex<Option<Phase>>,
    /// One-shot post-phase failure (S5c-X round 3): when the pipeline passes
    /// `phase` and this is `Some(phase)`, the pipeline returns
    /// `DbError::Storage("injected")`. Consumed on first fire — the flag does
    /// not fire again — UNLESS `fail_after_sticky` is set, in which case it
    /// stays armed (every retry fails too) until the test clears `fail_after`.
    pub fail_after: Mutex<Option<Phase>>,
    pub fail_after_sticky: AtomicBool,
    pub reached: Notify,
    pub resume: Notify,
}

fn hooks() -> &'static Mutex<TFxMap<String, Arc<Hook>>> {
    static HOOKS: OnceLock<Mutex<TFxMap<String, Arc<Hook>>>> = OnceLock::new();
    HOOKS.get_or_init(|| Mutex::new(TFxMap::default()))
}

#[cfg(feature = "fjall")]
pub(crate) fn install(repo_name: &str, hook: Arc<Hook>) {
    hooks().lock().unwrap().insert(repo_name.to_owned(), hook);
}

#[cfg(feature = "fjall")]
pub(crate) fn remove(repo_name: &str) {
    hooks().lock().unwrap().remove(repo_name);
}

pub(crate) fn barrier_action(repo_name: &str) -> (bool, bool) {
    let hooks = hooks().lock().unwrap();
    hooks.get(repo_name).map_or((false, false), |hook| {
        (
            hook.fail_journal_barrier.load(Ordering::Acquire),
            hook.bypass_journal_barrier.load(Ordering::Acquire),
        )
    })
}

pub(crate) fn fail_history_flush(repo_name: &str) -> bool {
    hooks()
        .lock()
        .unwrap()
        .get(repo_name)
        .is_some_and(|hook| hook.fail_history_flush.load(Ordering::Acquire))
}

/// Consume the one-shot post-`phase` failure flag for `repo_name` (armed
/// only when it holds exactly this phase). Returns `true` exactly when the
/// caller must return the injected `DbError::Storage("injected")`. Sticky
/// mode leaves the flag armed so every retry attempt fails too.
pub(crate) fn take_fail_after(repo_name: &str, phase: Phase) -> bool {
    let hooks = hooks().lock().unwrap();
    let Some(hook) = hooks.get(repo_name) else {
        return false;
    };
    let mut slot = hook.fail_after.lock().unwrap();
    if *slot != Some(phase) {
        return false;
    }
    if !hook.fail_after_sticky.load(Ordering::Acquire) {
        // One-shot: consumed by this firing.
        *slot = None;
    }
    true
}

#[cfg(test)]
pub(crate) async fn history_written(repo: &crate::repo::RepoInstance) {
    phase(repo.name(), Phase::HistoryWritten).await;
}

pub(crate) async fn phase(repo_name: &str, phase: Phase) {
    let hook = hooks().lock().unwrap().get(repo_name).cloned();
    if let Some(hook) = hook {
        let park = *hook.park_at.lock().unwrap() == Some(phase);
        if park {
            let resumed = hook.resume.notified();
            tokio::pin!(resumed);
            resumed.as_mut().enable();
            hook.reached.notify_waiters();
            resumed.await;
        }
        if *hook.abort_at.lock().unwrap() == Some(phase) {
            // The notify lets an in-process observer proceed before the
            // abort (kill without graceful shutdown, for the crash harness).
            hook.reached.notify_waiters();
            std::process::abort();
        }
    }
}
