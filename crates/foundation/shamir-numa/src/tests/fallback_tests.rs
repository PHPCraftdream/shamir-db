//! `FallbackSingleNodeTopology` — single UMA node, no-op pin.

use crate::{AffinityError, FallbackSingleNodeTopology, NodeId, Topology};

#[test]
fn reports_exactly_one_node() {
    let t = FallbackSingleNodeTopology::with_cpus(8);
    assert_eq!(t.num_nodes(), 1);
}

#[test]
fn node_zero_owns_every_cpu() {
    let t = FallbackSingleNodeTopology::with_cpus(4);
    assert_eq!(t.cores_on_node(NodeId(0)).len(), 4);
    // Out-of-range node yields an empty slice, never a panic.
    assert!(t.cores_on_node(NodeId(1)).is_empty());
}

#[test]
fn with_cpus_zero_still_has_one_cpu() {
    // A topology must own at least one CPU; 0 is clamped to 1.
    let t = FallbackSingleNodeTopology::with_cpus(0);
    assert_eq!(t.cores_on_node(NodeId(0)).len(), 1);
}

#[test]
fn current_node_is_always_zero() {
    let t = FallbackSingleNodeTopology::detect();
    assert_eq!(t.current_node(), NodeId(0));
}

#[test]
fn pin_to_node_zero_is_a_noop_success() {
    let t = FallbackSingleNodeTopology::with_cpus(2);
    assert!(t.pin_current_thread_to_node(NodeId(0)).is_ok());
}

#[test]
fn pin_to_other_node_is_out_of_range() {
    let t = FallbackSingleNodeTopology::with_cpus(2);
    let err = t.pin_current_thread_to_node(NodeId(1)).unwrap_err();
    assert!(matches!(
        err,
        AffinityError::NodeOutOfRange {
            requested: 1,
            available: 1
        }
    ));
}

#[test]
fn detect_reports_at_least_one_cpu() {
    let t = FallbackSingleNodeTopology::detect();
    assert!(!t.cores_on_node(NodeId(0)).is_empty());
}
