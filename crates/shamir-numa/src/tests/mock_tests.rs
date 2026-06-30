//! `MockTopology` — DI double semantics.

use crate::{AffinityError, CpuId, MockTopology, NodeId, Topology};

#[test]
fn reports_configured_node_count_and_cores() {
    let t = MockTopology::with_nodes(2, 4);
    assert_eq!(t.num_nodes(), 2);
    assert_eq!(
        t.cores_on_node(NodeId(0)),
        &[CpuId(0), CpuId(1), CpuId(2), CpuId(3)]
    );
    assert_eq!(
        t.cores_on_node(NodeId(1)),
        &[CpuId(4), CpuId(5), CpuId(6), CpuId(7)]
    );
    assert!(t.cores_on_node(NodeId(2)).is_empty());
}

#[test]
fn current_node_follows_thread_local_override() {
    // Run on a fresh thread so the thread-local starts clean and cannot leak
    // into / out of sibling tests sharing the harness thread pool.
    std::thread::spawn(|| {
        let t = MockTopology::with_nodes(4, 2);
        assert_eq!(t.current_node(), NodeId(0)); // default
        MockTopology::set_current_node_for_test(NodeId(3));
        assert_eq!(t.current_node(), NodeId(3));
    })
    .join()
    .unwrap();
}

#[test]
fn current_node_clamps_out_of_range_override() {
    std::thread::spawn(|| {
        let t = MockTopology::with_nodes(2, 2);
        MockTopology::set_current_node_for_test(NodeId(9)); // beyond this mock
        assert_eq!(t.current_node(), NodeId(0)); // clamped, no OOB
    })
    .join()
    .unwrap();
}

#[test]
fn pin_records_and_updates_current_node() {
    std::thread::spawn(|| {
        let t = MockTopology::with_nodes(3, 2);
        assert!(t.pin_current_thread_to_node(NodeId(2)).is_ok());
        // A pinned thread subsequently reports its node (mirrors getcpu).
        assert_eq!(t.current_node(), NodeId(2));
        let log = t.pin_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].1, NodeId(2));
        assert_eq!(log[0].0, std::thread::current().id());
    })
    .join()
    .unwrap();
}

#[test]
fn pin_out_of_range_errors_and_is_not_logged() {
    let t = MockTopology::with_nodes(2, 2);
    let err = t.pin_current_thread_to_node(NodeId(5)).unwrap_err();
    assert!(matches!(
        err,
        AffinityError::NodeOutOfRange {
            requested: 5,
            available: 2
        }
    ));
    assert!(t.pin_log().is_empty());
}

#[test]
#[should_panic(expected = "at least one node")]
fn zero_nodes_panics() {
    let _ = MockTopology::with_nodes(0, 4);
}
