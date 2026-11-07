//! Identifier newtype invariants.

use crate::{CpuId, NodeId};

#[test]
fn node_id_ordering_and_equality() {
    assert!(NodeId(0) < NodeId(1));
    assert_eq!(NodeId(2), NodeId(2));
    let mut v = [NodeId(3), NodeId(1), NodeId(2)];
    v.sort();
    assert_eq!(v, [NodeId(1), NodeId(2), NodeId(3)]);
}

#[test]
fn cpu_id_is_a_transparent_index() {
    assert_eq!(CpuId(7).0, 7);
    assert_eq!(CpuId(0), CpuId(0));
    assert_ne!(CpuId(0), CpuId(1));
}
