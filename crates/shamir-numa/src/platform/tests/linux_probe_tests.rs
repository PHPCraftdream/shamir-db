//! Linux topology discovery and affinity tests.

use std::io;

use crate::platform::linux::LinuxTopology;
use crate::{AffinityError, NodeId, Topology};

#[test]
fn rv_numa_c46d0e41_missing_and_syscall_errors_are_classified() {
    let missing = LinuxTopology::probe_with(|_| Err(io::Error::from(io::ErrorKind::NotFound)));
    assert!(matches!(missing, Err(AffinityError::Unsupported)));

    let denied =
        LinuxTopology::probe_with(|_| Err(io::Error::from(io::ErrorKind::PermissionDenied)));
    assert!(matches!(denied, Err(AffinityError::Syscall(_))));
}

#[test]
fn rv_numa_5cccbb1a_per_node_syscall_error_propagates_but_missing_is_empty() {
    let topology = LinuxTopology::probe_with(|path| match path {
        "/sys/devices/system/node/online" => Ok("0-1".to_owned()),
        p if p.ends_with("node0/cpulist") => Ok("0-2".to_owned()),
        _ => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
    });
    assert!(matches!(topology, Err(AffinityError::Syscall(_))));

    let topology = LinuxTopology::probe_with(|path| match path {
        "/sys/devices/system/node/online" => Ok("0-1".to_owned()),
        p if p.ends_with("node0/cpulist") => Ok("0-2".to_owned()),
        _ => Err(io::Error::from(io::ErrorKind::NotFound)),
    })
    .unwrap();
    assert!(topology.cores_on_node(NodeId(1)).is_empty());
}

#[test]
fn rv_numa_dc25aac1_sparse_nodes_are_valid_fixtures() {
    let topology = LinuxTopology::probe_with(|path| match path {
        "/sys/devices/system/node/online" => Ok("2,7".to_owned()),
        p if p.ends_with("node2/cpulist") => Ok("4-5".to_owned()),
        _ => Ok("".to_owned()),
    })
    .unwrap();
    assert_eq!(topology.num_nodes(), 2);
    assert!(topology.cores_on_node(NodeId(0)).contains(&crate::CpuId(4)));
    assert!(topology.cores_on_node(NodeId(1)).is_empty());
}

#[test]
fn rv_numa_dc25aac1_empty_first_node_is_a_valid_topology() {
    // The old host test asserted slot 0 is populated; a valid probe can leave it empty.
    let topology = LinuxTopology::probe_with(|path| match path {
        "/sys/devices/system/node/online" => Ok("0-1".to_owned()),
        p if p.ends_with("node1/cpulist") => Ok("2-3".to_owned()),
        _ => Ok("".to_owned()),
    })
    .unwrap();
    assert_eq!(topology.num_nodes(), 2);
    assert!(topology.cores_on_node(NodeId(0)).is_empty());
    assert_eq!(
        topology.cores_on_node(NodeId(1)),
        &[crate::CpuId(2), crate::CpuId(3)]
    );
}

#[test]
fn explicit_out_of_range_node_is_rejected_without_affinity_syscall() {
    let topology = LinuxTopology::probe_with(|path| match path {
        "/sys/devices/system/node/online" => Ok("7,2,7".to_owned()),
        p if p.ends_with("node2/cpulist") => Ok("4-5".to_owned()),
        _ => Ok("8-9".to_owned()),
    })
    .unwrap();
    assert_eq!(
        topology.cores_on_node(NodeId(0)),
        &[crate::CpuId(4), crate::CpuId(5)]
    );
    assert_eq!(
        topology.cores_on_node(NodeId(1)),
        &[crate::CpuId(8), crate::CpuId(9)]
    );
    assert!(topology.cores_on_node(NodeId(2)).is_empty());
    assert!(matches!(
        topology.pin_current_thread_to_node(NodeId(2)),
        Err(AffinityError::NodeOutOfRange {
            requested: 2,
            available: 2
        })
    ));
}

#[test]
fn probe_on_real_linux_host_succeeds() {
    let topo = LinuxTopology::probe().expect("Linux host must expose /sys/devices/system/node");
    assert!(topo.num_nodes() >= 1);
}

#[test]
fn current_node_is_in_range() {
    let topo = LinuxTopology::probe().unwrap();
    let n = topo.current_node();
    assert!(n.0 < topo.num_nodes());
}

#[test]
fn rv_numa_3378686a_rejects_cpu_outside_fixed_set_before_cpu_set() {
    let cpu = (libc::CPU_SETSIZE as usize).to_string();
    let topology = LinuxTopology::probe_with(|path| match path {
        "/sys/devices/system/node/online" => Ok("0".to_owned()),
        _ => Ok(cpu.clone()),
    })
    .unwrap();
    assert!(matches!(
        topology.pin_current_thread_to_node(NodeId(0)),
        Err(AffinityError::Unsupported)
    ));
}
