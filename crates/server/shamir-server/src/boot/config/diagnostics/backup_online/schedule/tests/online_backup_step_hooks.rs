//! Test-only engine barrier-step hook plumbing for the online backup
//! (R2a-4), moved out of the registry for layout and declared by `#[path]`
//! from `schedule` as its OWN `#[cfg(test)]` module (never inside `tests`,
//! whose HEAD visibility must not change). This holds THE shared
//! dispatcher this test binary installs into the engine and the per-thread
//! arming helpers; the registry re-exports them (`registry::arm_backup_step`
//! et al.) so every existing call site keeps its path.

use std::cell::RefCell;

use shamir_db::engine::repo::RepoInstance;

/// Test-only step callback: invoked by the engine's barrier-step hook
/// with the step name, on the armed runner thread.
pub(crate) type BackupStepCallback = Box<dyn Fn(&'static str) + Send>;

// THE shared engine-step dispatcher for this test binary (R2a-4): the
// engine's `ONLINE_BACKUP_STEP_HOOK` slot is a first-wins
// `fn(&'static str)` `OnceLock`, so every test in this binary that wants
// barrier-step callbacks must go through this one function instead of
// installing its own hook (two competing installers would silently
// break the loser). Dispatch is per-THREAD: the hook is a no-op on any
// thread that has not armed a callback, so arming is per-run/per-thread
// state — never process-global.
thread_local! {
    static TEST_STEP_CALLBACK: RefCell<Option<BackupStepCallback>> = const { RefCell::new(None) };
}

fn shared_test_step_hook(step: &'static str) {
    TEST_STEP_CALLBACK.with(|slot| {
        if let Some(callback) = slot.borrow().as_ref() {
            callback(step);
        }
    });
}

/// Install the shared dispatcher into the engine. The engine's setter is
/// itself idempotent (first install wins), so this is safe to call from
/// every arming site.
pub(crate) fn install_test_step_hook() {
    RepoInstance::set_online_backup_step_hook(shared_test_step_hook);
}

/// Arm the CURRENT thread with `callback` verbatim — the registry's per-run
/// install path, which moves a run's one-shot callback onto its runner
/// thread. Never process-global.
pub(crate) fn set_thread_step_callback(callback: BackupStepCallback) {
    TEST_STEP_CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
}

/// Arm the CURRENT thread: `on_step` fires exactly once when the engine's
/// barrier reaches `step` on this thread (one-shot; later steps run
/// unimpeded). Callable from any test that drives its own backup/bridge
/// thread — arm inside the driving thread before the barrier runs. Never
/// process-global.
pub(crate) fn arm_backup_step(step: &'static str, on_step: impl FnOnce() + Send + 'static) {
    install_test_step_hook();
    let fired_once = RefCell::new(Some(on_step));
    TEST_STEP_CALLBACK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |fired| {
            if fired == step {
                if let Some(on_step) = fired_once.borrow_mut().take() {
                    on_step();
                }
            }
        }));
    });
}
