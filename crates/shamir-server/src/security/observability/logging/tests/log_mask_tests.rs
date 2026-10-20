use crate::logging::{ns, LogMask};
use tracing::level_filters::LevelFilter;
use tracing::Level;

#[test]
fn log_mask_default_allows_info_and_above() {
    let mask = LogMask::new(LevelFilter::INFO);

    assert!(mask.allows("anything", &Level::INFO));
    assert!(mask.allows("anything", &Level::WARN));
    assert!(mask.allows("anything", &Level::ERROR));
    assert!(!mask.allows("anything", &Level::DEBUG));
    assert!(!mask.allows("anything", &Level::TRACE));
}

#[test]
fn log_mask_override_allows_trace_on_specific_target() {
    let mask = LogMask::new(LevelFilter::INFO).with_override("wal", LevelFilter::TRACE);

    // Override: "wal" at TRACE → TRACE is allowed.
    assert!(mask.allows("wal", &Level::TRACE));
    assert!(mask.allows("wal", &Level::DEBUG));
    assert!(mask.allows("wal", &Level::INFO));

    // Default: other targets still deny TRACE/DEBUG.
    assert!(!mask.allows("tx", &Level::TRACE));
    assert!(!mask.allows("tx", &Level::DEBUG));
    assert!(mask.allows("tx", &Level::INFO));
}

#[test]
fn log_mask_longest_prefix_wins() {
    let mask = LogMask::new(LevelFilter::WARN)
        .with_override("shamir", LevelFilter::INFO)
        .with_override("shamir_engine", LevelFilter::TRACE);

    // "shamir_engine::tx" matches both "shamir" and "shamir_engine";
    // longest prefix ("shamir_engine") wins → TRACE allowed.
    assert!(mask.allows("shamir_engine::tx", &Level::TRACE));

    // "shamir_storage::kv" matches only "shamir" → INFO allowed, TRACE not.
    assert!(mask.allows("shamir_storage::kv", &Level::INFO));
    assert!(!mask.allows("shamir_storage::kv", &Level::TRACE));

    // Unrelated target → default WARN.
    assert!(mask.allows("other", &Level::WARN));
    assert!(!mask.allows("other", &Level::INFO));
}

#[test]
fn log_mask_exact_match_beats_shorter_prefix() {
    let mask = LogMask::new(LevelFilter::ERROR)
        .with_override("wal", LevelFilter::DEBUG)
        .with_override("wal_sync", LevelFilter::TRACE);

    // Exact match on "wal" → DEBUG.
    assert!(mask.allows("wal", &Level::DEBUG));
    assert!(!mask.allows("wal", &Level::TRACE));

    // "wal_sync" → TRACE (longest prefix).
    assert!(mask.allows("wal_sync", &Level::TRACE));

    // "wal_compact" → "wal" prefix match → DEBUG.
    assert!(mask.allows("wal_compact", &Level::DEBUG));
    assert!(!mask.allows("wal_compact", &Level::TRACE));
}

#[test]
fn log_mask_override_replaces_existing() {
    let mask = LogMask::new(LevelFilter::INFO)
        .with_override("wal", LevelFilter::TRACE)
        .with_override("wal", LevelFilter::WARN);

    // Second override replaces the first.
    assert!(!mask.allows("wal", &Level::TRACE));
    assert!(!mask.allows("wal", &Level::INFO));
    assert!(mask.allows("wal", &Level::WARN));
}

#[test]
fn log_mask_off_disables_everything() {
    let mask = LogMask::new(LevelFilter::OFF);
    assert!(!mask.allows("anything", &Level::ERROR));
    assert!(!mask.allows("anything", &Level::WARN));
    assert!(!mask.allows("anything", &Level::INFO));
}

#[test]
fn log_mask_with_all_namespace_targets() {
    let mask = LogMask::new(LevelFilter::WARN)
        .with_override(ns::WAL, LevelFilter::TRACE)
        .with_override(ns::ENGINE, LevelFilter::DEBUG);

    assert!(mask.allows(ns::WAL, &Level::TRACE));
    assert!(mask.allows(ns::ENGINE, &Level::DEBUG));
    assert!(!mask.allows(ns::TX, &Level::DEBUG));
    assert!(mask.allows(ns::TX, &Level::WARN));
}
