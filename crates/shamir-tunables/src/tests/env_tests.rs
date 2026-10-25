//! Regression test RV-TUNABLES-A3D3695D — env override must actually be read
//! (pure parse layer; process-global env mutation deliberately avoided).

use crate::instance_defaults::{
    parse_vector_snapshot_delta_threshold, VECTOR_SNAPSHOT_DELTA_THRESHOLD,
    VECTOR_SNAPSHOT_DELTA_THRESHOLD_ENV,
};

#[test]
fn rv_tunables_a3d3695d_vector_snapshot_delta_threshold_env_parse() {
    assert_eq!(parse_vector_snapshot_delta_threshold(Some("5000")), 5000);
    assert_eq!(parse_vector_snapshot_delta_threshold(Some(" 7000 ")), 7000);
    assert_eq!(
        parse_vector_snapshot_delta_threshold(Some("bogus")),
        VECTOR_SNAPSHOT_DELTA_THRESHOLD
    );
    assert_eq!(
        parse_vector_snapshot_delta_threshold(None),
        VECTOR_SNAPSHOT_DELTA_THRESHOLD
    );
    assert_eq!(VECTOR_SNAPSHOT_DELTA_THRESHOLD, 10_000);
    assert_eq!(
        VECTOR_SNAPSHOT_DELTA_THRESHOLD_ENV,
        "SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD"
    );
}
