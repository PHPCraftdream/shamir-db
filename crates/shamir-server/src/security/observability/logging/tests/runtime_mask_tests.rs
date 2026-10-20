use crate::logging::{current_mask, set_mask, set_namespace_level, LogMask};
use serial_test::serial;
use tracing::level_filters::LevelFilter;
use tracing::Level;

#[test]
#[serial]
fn set_namespace_level_takes_effect_live() {
    // Boot with INFO default — DEBUG on "wal" is denied.
    set_mask(LogMask::new(LevelFilter::INFO));
    assert!(!current_mask().allows("wal", &Level::DEBUG));

    // Promote "wal" to DEBUG at runtime — lock-free swap.
    set_namespace_level("wal", LevelFilter::DEBUG);
    let updated = current_mask();
    assert!(
        updated.allows("wal", &Level::DEBUG),
        "wal should allow DEBUG after set_namespace_level"
    );
    assert!(
        updated.allows("wal", &Level::INFO),
        "wal should still allow INFO"
    );

    // Other targets unaffected.
    assert!(
        !updated.allows("tx", &Level::DEBUG),
        "tx should still deny DEBUG"
    );
    assert!(
        updated.allows("tx", &Level::INFO),
        "tx should still allow INFO"
    );
}

#[test]
#[serial]
fn set_mask_replaces_entire_mask() {
    set_mask(LogMask::new(LevelFilter::TRACE));
    assert!(current_mask().allows("anything", &Level::TRACE));

    set_mask(LogMask::new(LevelFilter::ERROR));
    assert!(!current_mask().allows("anything", &Level::WARN));
    assert!(current_mask().allows("anything", &Level::ERROR));
}

#[test]
#[serial]
fn current_mask_snapshot_is_consistent() {
    set_mask(LogMask::new(LevelFilter::INFO).with_override("wal", LevelFilter::TRACE));
    let snap = current_mask();

    // Swap the global — snapshot should remain unchanged (Arc semantics).
    set_mask(LogMask::new(LevelFilter::ERROR));
    assert!(snap.allows("wal", &Level::TRACE));
    assert!(snap.allows("anything", &Level::INFO));

    // New snapshot reflects the update.
    let fresh = current_mask();
    assert!(!fresh.allows("anything", &Level::INFO));
}
