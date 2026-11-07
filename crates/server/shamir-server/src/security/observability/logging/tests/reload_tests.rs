use crate::config::LoggingConfig;
use crate::logging::{current_mask, reload, set_mask, LogMask};
use serial_test::serial;
use tracing::level_filters::LevelFilter;
use tracing::Level;

/// Proves the lock-free live reload: `reload(&LoggingConfig)` applies a new
/// global log level via the `ArcSwap<LogMask>` without recreating the
/// subscriber or writer. The SIGHUP signal wiring itself is not
/// unit-testable (requires a running process + signal delivery).
#[test]
#[serial]
fn reload_applies_new_level() {
    // Start at INFO — DEBUG should be denied.
    set_mask(LogMask::new(LevelFilter::INFO));
    assert!(
        !current_mask().allows("x", &Level::DEBUG),
        "DEBUG must be denied at INFO level"
    );

    // Reload with "debug" — DEBUG should now be allowed.
    reload(&LoggingConfig {
        level: "debug".into(),
        ..Default::default()
    });
    assert!(
        current_mask().allows("x", &Level::DEBUG),
        "DEBUG must be allowed after reload to debug"
    );
    assert!(
        current_mask().allows("x", &Level::INFO),
        "INFO must still be allowed after reload to debug"
    );

    // Reload back to "warn" — INFO should be denied, WARN allowed.
    reload(&LoggingConfig {
        level: "warn".into(),
        ..Default::default()
    });
    assert!(
        !current_mask().allows("x", &Level::INFO),
        "INFO must be denied after reload to warn"
    );
    assert!(
        current_mask().allows("x", &Level::WARN),
        "WARN must be allowed after reload to warn"
    );
}
