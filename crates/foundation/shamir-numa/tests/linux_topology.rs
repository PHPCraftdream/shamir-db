#![cfg(target_os = "linux")]
//! Integration tests for [`LinuxTopology`] — compiled and run only on Linux.
//!
//! These tests require a real Linux host with sysfs mounted at
//! `/sys/devices/system/node/`. They are excluded from the Windows dev-host
//! build by the `#![cfg(target_os = "linux")]` crate attribute above.

use shamir_numa::{detect, NodeId};

#[test]
fn detect_returns_non_empty_topology() {
    let topo = detect();
    assert!(topo.num_nodes() >= 1);
}

#[test]
fn pin_to_node_zero_succeeds() {
    let topo = detect();
    // SAFETY: `eligible` is a valid, zero-initialized cpu_set_t and its size matches the syscall ABI.
    let mut eligible: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is valid for the duration of the syscall and the size is exact.
    unsafe {
        libc::CPU_ZERO(&mut eligible);
        assert_eq!(
            libc::sched_getaffinity(0, std::mem::size_of_val(&eligible), &mut eligible),
            0
        );
    }
    // If discovery has no CPUs eligible to this thread (for example in a
    // restricted container), there is no valid node to pin; skip this host oracle.
    let Some(node) = (0..topo.num_nodes()).find(|&slot| {
        topo.cores_on_node(NodeId(slot)).iter().any(|cpu| {
            // SAFETY: cpu IDs are checked below CPU_SETSIZE and eligible is initialized.
            cpu.0 < libc::CPU_SETSIZE as usize && unsafe { libc::CPU_ISSET(cpu.0, &eligible) }
        })
    }) else {
        return;
    };
    topo.pin_current_thread_to_node(NodeId(node))
        .expect("pinning to an eligible node must succeed");
}
