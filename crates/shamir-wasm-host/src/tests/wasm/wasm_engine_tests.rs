//! Engine lifetime: the epoch ticker must not keep a dropped engine alive.

use crate::WasmEngine;

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
