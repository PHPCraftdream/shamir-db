//! Engine lifetime: the epoch ticker must not keep a dropped engine alive.

use crate::WasmEngine;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn rv_wasm_host_fa126f4a_epoch_ticker_spawn_failure_is_returned() {
    let called = AtomicBool::new(false);
    let result = WasmEngine::build_with_ticker(None, |_| {
        called.store(true, Ordering::SeqCst);
        Err(crate::FunctionError::Compute(
            "injected epoch ticker spawn failure".into(),
        ))
    });

    assert!(
        called.load(Ordering::SeqCst),
        "ticker startup was not attempted"
    );
    match result {
        Err(crate::FunctionError::Compute(message)) => {
            assert!(message.contains("injected epoch ticker spawn failure"));
        }
        Err(other) => panic!("unexpected error: {other:?}"),
        Ok(_) => panic!("expected engine construction failure"),
    }
}

#[test]
fn epoch_ticker_does_not_keep_dropped_engine_alive() {
    let engine = WasmEngine::new().unwrap();
    let weak = engine.engine().weak();
    assert!(weak.upgrade().is_some(), "engine alive while owned");
    drop(engine);
    // A ticker holding a strong clone would keep the inner engine alive forever.
    assert!(
        weak.upgrade().is_none(),
        "epoch ticker thread retains the dropped engine"
    );
}
