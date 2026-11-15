use std::sync::{Arc, Mutex};

use crate::tx::commit::core::RepoCore;

#[derive(Default)]
pub(crate) struct State {
    pub(crate) fail_checkpoint: bool,
    pub(crate) bypass_checkpoint: bool,
    pub(crate) order: Vec<&'static str>,
    pub(crate) on_sample_w: Option<Box<dyn FnOnce() + Send>>,
}

pub(crate) fn with_state<T>(repo: &RepoCore, action: impl FnOnce(&mut State) -> T) -> T {
    let slot = repo.clean_horizon_test_state();
    let state = slot.get_or_init(|| Arc::new(Mutex::new(State::default())));
    action(&mut state.lock().expect("test seam mutex poisoned"))
}

pub(crate) fn bypass(repo: &RepoCore) -> bool {
    with_state(repo, |state| state.bypass_checkpoint)
}

pub(crate) fn should_fail(repo: &RepoCore) -> bool {
    with_state(repo, |state| state.fail_checkpoint)
}

pub(crate) fn record(repo: &RepoCore, phase: &'static str) {
    let callback = with_state(repo, |state| {
        state.order.push(phase);
        if phase == "sampled_w" {
            state.on_sample_w.take()
        } else {
            None
        }
    });
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(all(test, feature = "fjall"))]
pub(crate) fn on_sample_w(repo: &RepoCore, callback: impl FnOnce() + Send + 'static) {
    with_state(repo, |state| state.on_sample_w = Some(Box::new(callback)));
}

#[cfg(all(test, feature = "fjall"))]
pub(crate) fn take_order(repo: &RepoCore) -> Vec<&'static str> {
    with_state(repo, |state| std::mem::take(&mut state.order))
}
