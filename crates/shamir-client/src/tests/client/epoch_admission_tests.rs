use crate::client::epoch_admission::should_advertise_interner_epochs;

#[test]
fn rv_client_1c219006_legacy_server_skips_advertisement() {
    assert!(!should_advertise_interner_epochs(1, false));
    assert!(!should_advertise_interner_epochs(1, true));
}

#[test]
fn rv_client_1c219006_v2_cold_cache_still_advertises() {
    // The cold-v2 path is intentionally retained for synchronization.
    assert!(should_advertise_interner_epochs(2, false));
    assert!(!should_advertise_interner_epochs(2, true));
}
