#![cfg(target_os = "linux")]
//! Integration tests for [`LinuxTopology`] — compiled and run only on Linux.
//!
//! These tests require a real Linux host with sysfs mounted at
//! `/sys/devices/system/node/`. They are excluded from the Windows dev-host
//! build by the `#![cfg(target_os = "linux")]` crate attribute above.

use shamir_numa::detect;

#[test]
fn detect_returns_non_empty_topology() {
    let topo = detect();
    assert!(topo.num_nodes() >= 1);
}

#[test]
fn pin_to_node_zero_succeeds() {
    let topo = detect();
    topo.pin_current_thread_to_node(shamir_numa::NodeId(0))
        .expect("pin to node 0 should succeed on any Linux host");
}
