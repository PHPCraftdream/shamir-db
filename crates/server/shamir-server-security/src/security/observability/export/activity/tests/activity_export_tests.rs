//! Gauge/mirror agreement tests for the R4b-1 activity export: the
//! rendered `shamir_active_*` series must match the sampled
//! [`ActivityMetricsSnapshot`] exactly, carry the four names with HELP and
//! TYPE gauge descriptors, stay unlabeled, and render explicit zeros for
//! a server without an installed source. Deterministic: pure text
//! rendering, no global recorder, no clock.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::super::{render_activity_series, ActivityMetricsSnapshot, ActivitySource};

const GAUGE_NAMES: [&str; 4] = [
    "shamir_active_requests",
    "shamir_active_sessions",
    "shamir_active_transactions",
    "shamir_active_cursors",
];

/// Parse a rendered series block back into `(name -> value)`, failing on
/// anything that is not an unlabeled sample line.
fn parse_samples(rendered: &str) -> BTreeMap<&str, u64> {
    let mut samples = BTreeMap::new();
    for line in rendered.lines() {
        assert!(!line.contains('{'), "unlabeled gauges only: {line}");
        if let Some(rest) = line.strip_prefix("# ") {
            assert!(
                rest.starts_with("HELP ") || rest.starts_with("TYPE "),
                "unexpected comment line: {line}"
            );
            continue;
        }
        let (name, value) = line
            .split_once(' ')
            .unwrap_or_else(|| panic!("sample line must be `name value`: {line}"));
        let value: u64 = value
            .parse()
            .unwrap_or_else(|e| panic!("sample value for {name} is not an integer: {e}"));
        assert!(
            samples.insert(name, value).is_none(),
            "duplicate series descriptor for {name}"
        );
    }
    samples
}

/// Gauge agreement across the exporter: every rendered sample value
/// equals the snapshot field it mirrors, and each series carries exactly
/// one HELP and one TYPE gauge descriptor.
#[test]
fn rendered_series_matches_snapshot_values() {
    let snapshot = ActivityMetricsSnapshot {
        requests: 3,
        sessions: 7,
        transactions: 1,
        cursors: 12,
    };
    let rendered = render_activity_series(&snapshot);
    let samples = parse_samples(&rendered);

    assert_eq!(samples.len(), 4, "exactly four series:\n{rendered}");
    for name in GAUGE_NAMES {
        let value = samples
            .get(name)
            .unwrap_or_else(|| panic!("missing series {name}:\n{rendered}"));
        let help_lines = rendered
            .lines()
            .filter(|line| line.starts_with(&format!("# HELP {name} ")))
            .count();
        let type_lines = rendered
            .lines()
            .filter(|line| line.starts_with(&format!("# TYPE {name} ")))
            .count();
        assert_eq!(help_lines, 1, "exactly one HELP descriptor for {name}");
        assert_eq!(type_lines, 1, "exactly one TYPE descriptor for {name}");
        assert!(
            rendered.contains(&format!("# TYPE {name} gauge\n")),
            "{name} must be a gauge:\n{rendered}"
        );
        let expected = match name {
            "shamir_active_requests" => snapshot.requests,
            "shamir_active_sessions" => snapshot.sessions,
            "shamir_active_transactions" => snapshot.transactions,
            _ => snapshot.cursors,
        };
        assert_eq!(*value, expected, "gauge {name} disagrees with the mirror");
    }
}

/// A server without an installed source renders explicit zeros, so a
/// Prometheus scraper sees the whole family from the first scrape.
#[test]
fn default_snapshot_renders_zero_gauges() {
    let rendered = render_activity_series(&ActivityMetricsSnapshot::default());
    let samples = parse_samples(&rendered);
    assert_eq!(samples.len(), 4);
    for name in GAUGE_NAMES {
        assert_eq!(samples[name], 0, "zero-touch series {name} must be 0");
    }
}

/// Each [`ActivitySource`] dispatches `sample()` to its own installed
/// closure and sources stay independent: two sources built from different
/// closures sample different snapshots (and render different series), so
/// a per-state source can never bleed another server's counts into a
/// scrape.
#[test]
fn distinct_sources_sample_their_own_closure() {
    let make_source = |requests: u64, sessions: u64| {
        ActivitySource::new(Arc::new(move || ActivityMetricsSnapshot {
            requests,
            sessions,
            transactions: 0,
            cursors: 0,
        }))
    };
    let first = make_source(9, 4);
    let second = make_source(2, 8);

    for (source, expected_requests, expected_sessions) in [(&first, 9, 4), (&second, 2, 8)] {
        let snapshot = source.sample();
        assert_eq!(snapshot.requests, expected_requests);
        assert_eq!(snapshot.sessions, expected_sessions);
        assert_eq!(snapshot.transactions, 0);
        assert_eq!(snapshot.cursors, 0);
    }
    assert_ne!(
        render_activity_series(&first.sample()),
        render_activity_series(&second.sample()),
        "independent sources must render distinct series"
    );
}
