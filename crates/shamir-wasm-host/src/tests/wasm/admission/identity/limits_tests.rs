use crate::{AdmissionLimits, WasmEngineOptions};

#[test]
fn engine_options_default_carries_default_admission_limits() {
    assert_eq!(
        WasmEngineOptions::default().admission,
        AdmissionLimits::default()
    );
}
