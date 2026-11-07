use crate::{AdmissionLimits, WasmEngineOptions};
use std::time::Duration;

const MIB: usize = 1024 * 1024;
const FRAME_CAP: usize = 16 * MIB;

#[test]
fn defaults_match_the_documented_table() {
    let l = AdmissionLimits::default();
    assert_eq!(l.max_module_bytes, 8 * MIB);
    assert_eq!(l.max_functions, 20_000);
    assert_eq!(l.max_types, 5_000);
    assert_eq!(l.max_imports, 64);
    assert_eq!(l.max_exports, 64);
    assert_eq!(l.max_globals, 1_000);
    assert_eq!(l.max_locals_per_function, 10_000);
    assert_eq!(l.max_function_body_bytes, MIB);
    assert_eq!(l.max_operators_per_function, 500_000);
    assert_eq!(l.max_block_depth, 256);
    assert_eq!(l.max_br_table_targets, 4_096);
    assert_eq!(l.max_params_and_results, 128);
    assert_eq!(l.max_table_elements, 100_000);
    assert_eq!(l.max_element_segments, 1_000);
    assert_eq!(l.max_element_items_total, 200_000);
    assert_eq!(l.max_data_segments, 10_000);
    assert_eq!(l.max_data_bytes_total, 4 * MIB);
    assert_eq!(l.max_memory_min_pages, 1_024);
    assert_eq!(l.max_source_bytes, MIB);
    assert_eq!(l.max_compile_time, Duration::from_secs(10));
    assert_eq!(
        l.max_concurrent_compiles,
        std::thread::available_parallelism()
            .map_or(1, |cores| cores.get() / 2)
            .max(1)
    );
}

#[test]
fn engine_options_default_carries_default_admission_limits() {
    assert_eq!(
        WasmEngineOptions::default().admission,
        AdmissionLimits::default()
    );
}

#[test]
fn defaults_fit_a_16_mib_frame() {
    assert!(AdmissionLimits::default()
        .validate_against_frame(FRAME_CAP)
        .is_ok());
}

#[test]
fn module_cap_equal_to_frame_rejected() {
    let limits = AdmissionLimits {
        max_module_bytes: FRAME_CAP,
        ..AdmissionLimits::default()
    };
    assert!(limits.validate_against_frame(FRAME_CAP).is_err());
}

#[test]
fn module_cap_of_12_mib_rejected_for_16_mib_frame() {
    let limits = AdmissionLimits {
        max_module_bytes: 12 * MIB,
        ..AdmissionLimits::default()
    };
    assert!(limits.validate_against_frame(FRAME_CAP).is_err());
}

#[test]
fn frame_bound_is_strict() {
    // 3_000_000 bytes encode to exactly 4_000_000 base64 bytes.
    let limits = AdmissionLimits {
        max_module_bytes: 3_000_000,
        ..AdmissionLimits::default()
    };
    let needed = 4_000_000 + limits.max_source_bytes + 64 * 1024;
    assert!(limits.validate_against_frame(needed).is_err());
    assert!(limits.validate_against_frame(needed + 1).is_ok());
}

#[test]
fn frame_error_mentions_the_frame_cap() {
    let message = AdmissionLimits::default()
        .validate_against_frame(MIB)
        .unwrap_err();
    assert!(message.contains("frame cap"), "{message}");
}
