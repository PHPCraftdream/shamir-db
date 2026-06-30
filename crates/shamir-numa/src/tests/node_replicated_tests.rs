//! `NodeReplicated<T>` — replication semantics across the mock + fallback
//! topologies.

use std::sync::Arc;

use crate::{FallbackSingleNodeTopology, MockTopology, NodeId, NodeReplicated, Topology};

#[test]
fn single_node_degrades_to_one_replica() {
    let topo: Arc<dyn Topology> = Arc::new(FallbackSingleNodeTopology::with_cpus(4));
    let r = NodeReplicated::new(topo, vec![1u32, 2, 3]);
    assert_eq!(r.num_replicas(), 1);
    assert_eq!(&**r.load_local(), &vec![1, 2, 3]);
}

#[test]
fn multi_node_has_one_replica_per_node() {
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(4, 2));
    let r = NodeReplicated::new(topo, 0u64);
    assert_eq!(r.num_replicas(), 4);
}

#[test]
fn store_publishes_to_every_replica() {
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(3, 2));
    let r = NodeReplicated::new(topo, 0u64);
    r.store(99);
    for n in 0..3 {
        assert_eq!(**r.load_node(NodeId(n)), 99);
    }
}

#[test]
fn rcu_updates_all_replicas() {
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(4, 2));
    let r = NodeReplicated::new(topo, 0u64);
    r.rcu(|v| v + 1);
    r.rcu(|v| v + 41);
    for n in 0..4 {
        assert_eq!(**r.load_node(NodeId(n)), 42);
    }
}

#[test]
fn rcu_on_single_node_matches_bare_arcswap() {
    // The degenerate path: one replica, no mirror loop, plain CAS update.
    let topo: Arc<dyn Topology> = Arc::new(FallbackSingleNodeTopology::with_cpus(2));
    let r = NodeReplicated::new(topo, vec![10u32]);
    r.rcu(|cur| {
        let mut v = cur.clone();
        v.push(20);
        v
    });
    assert_eq!(&**r.load_local(), &vec![10, 20]);
}

#[test]
fn load_local_indexes_by_current_node() {
    // Diverge the replicas with store_node, then prove load_local reads the
    // cell selected by the calling thread's current node.
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(3, 2));
    let r = NodeReplicated::new(topo, 0u64);
    r.store_node(NodeId(0), 100);
    r.store_node(NodeId(1), 200);
    r.store_node(NodeId(2), 300);

    // Each assertion runs on its own thread with a known current-node so the
    // mock's thread-local override is isolated and deterministic.
    let r = Arc::new(r);
    for (node, expected) in [(0usize, 100u64), (1, 200), (2, 300)] {
        let r = Arc::clone(&r);
        std::thread::spawn(move || {
            MockTopology::set_current_node_for_test(NodeId(node));
            assert_eq!(**r.load_local(), expected);
        })
        .join()
        .unwrap();
    }
}

#[test]
fn load_local_clamps_when_current_node_out_of_range() {
    // current_node is clamped by the mock, but guard the replica index too:
    // even a hostile topology reporting an OOB node must not panic.
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(2, 2));
    let r = NodeReplicated::new(topo, 7u64);
    std::thread::spawn(move || {
        MockTopology::set_current_node_for_test(NodeId(99));
        assert_eq!(**r.load_local(), 7); // node clamps to 0, no OOB
    })
    .join()
    .unwrap();
}

#[test]
fn concurrent_rcu_does_not_lose_updates_on_node_zero() {
    // The node-0 CAS loop must serialise concurrent increments. With N threads
    // each doing K increments, the final node-0 value is exactly N*K.
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(2, 2));
    let r = Arc::new(NodeReplicated::new(topo, 0u64));
    let threads = 8;
    let per = 1000;
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let r = Arc::clone(&r);
            std::thread::spawn(move || {
                for _ in 0..per {
                    r.rcu(|v| v + 1);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(**r.load_node(NodeId(0)), threads * per);
}

#[test]
fn rcu_closure_sees_latest_value() {
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(2, 2));
    let r = NodeReplicated::new(topo, String::from("a"));
    r.rcu(|s| format!("{s}b"));
    r.rcu(|s| format!("{s}c"));
    assert_eq!(r.load_node(NodeId(1)).as_str(), "abc");
}
