//! R4b-1 follow-up: `security.query_limits.max_in_flight_requests_per_session`
//! must reject `0` at startup — zero admits nothing, it is not a disable
//! switch (see `QueryLimitsConfig::max_in_flight_requests_per_session`'s doc
//! comment and `Config::validate`) — and, when the key is OMITTED, must
//! resolve to `shamir_tunables::instance_defaults::CONN_MAX_IN_FLIGHT`.

use crate::config::{Config, ConfigError};

const EXAMPLE: &str = include_str!("../../../../../../../deploy/server.example.ktav");

#[test]
fn example_profiles_admit_a_positive_per_session_in_flight_cap() {
    let cfg: Config = ktav::from_str(EXAMPLE).unwrap();
    assert!(
        cfg.security.query_limits.max_in_flight_requests_per_session >= 1,
        "shipped example must not ship a zero cap"
    );
    cfg.validate().unwrap();
}

/// The shipped example does NOT set the key; the resolved value must be
/// exactly `CONN_MAX_IN_FLIGHT`, so the session-level cap and the
/// per-connection transport semaphore start in lockstep.
#[test]
fn omitted_field_resolves_to_conn_max_in_flight() {
    assert!(
        !EXAMPLE.contains("max_in_flight_requests_per_session"),
        "example is expected to omit the key; update this test if it ever sets it"
    );
    let cfg: Config = ktav::from_str(EXAMPLE).unwrap();
    assert_eq!(
        cfg.security.query_limits.max_in_flight_requests_per_session,
        shamir_tunables::instance_defaults::CONN_MAX_IN_FLIGHT,
        "omitted field must default to CONN_MAX_IN_FLIGHT"
    );
}

/// A nonzero explicit value in the file is accepted and kept verbatim.
#[test]
fn explicit_nonzero_field_is_accepted() {
    let text = EXAMPLE.replacen(
        "query_limits: {",
        "query_limits: { max_in_flight_requests_per_session: 17",
        1,
    );
    assert_ne!(text, EXAMPLE, "injection point must exist in the example");
    let cfg: Config = ktav::from_str(&text).unwrap();
    assert_eq!(
        cfg.security.query_limits.max_in_flight_requests_per_session,
        17
    );
    cfg.validate().unwrap();
}

#[test]
fn zero_per_session_in_flight_cap_is_a_validation_error() {
    let mut cfg: Config = ktav::from_str(EXAMPLE).unwrap();
    cfg.security.query_limits.max_in_flight_requests_per_session = 0;
    match cfg.validate() {
        Err(ConfigError::Validation(reason)) => {
            assert!(
                reason.contains("max_in_flight_requests_per_session"),
                "error must name the offending field: {reason}"
            );
        }
        other => panic!("expected ConfigError::Validation, got {other:?}"),
    }
    // And the cap one above zero round-trips fine.
    cfg.security.query_limits.max_in_flight_requests_per_session = 1;
    cfg.validate().unwrap();
}
