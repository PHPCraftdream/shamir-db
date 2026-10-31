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
fn current_node_mock_clamps_out_of_range_override() {
    // This pins MockTopology's current_node() behavior only; explicit replica
    // APIs have their own out-of-range coverage below.
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
fn explicit_node_access_out_of_range_falls_back_to_node_zero_rv_numa_553cb81d() {
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(2, 2));
    let r = NodeReplicated::new(topo, 7u64);
    r.store_node(NodeId(0), 10);
    r.store_node(NodeId(1), 20);

    for node in [NodeId(2), NodeId(usize::MAX)] {
        assert_eq!(
            **r.load_node(node),
            10,
            "out-of-range load must select node 0"
        );
        r.store_node(node, 30);
        assert_eq!(**r.load_node(NodeId(0)), 30);
        assert_eq!(**r.load_node(NodeId(1)), 20);
        r.store_node(NodeId(0), 10);
    }
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

#[test]
fn rcu_mirror_does_not_regress_newer_replica_rv_numa_6e1761a2() {
    // Regression: a delayed older mirror must never overwrite a newer value
    // on a non-zero node. Deterministic oracle: the one-shot gate parks the
    // first mirror AFTER it captured node 0's value (1) and BEFORE it stores
    // it into a replica. A newer full publish (1000) completes meanwhile; on
    // release the parked mirror stores its stale capture last. Unfixed code
    // (capture once, store, done) leaves nodes 1..3 at 1 → assert fails;
    // fixed code re-reads node 0 after the store, sees it moved, and
    // converges to 1000.
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(4, 2));
    let r = Arc::new(NodeReplicated::new(topo, 0u64));
    r.arm_mirror_gate();
    let publisher = {
        let r = Arc::clone(&r);
        std::thread::spawn(move || {
            r.rcu(|v| v + 1); // node 0 = 1, then pauses pre-mirror
        })
    };
    r.wait_mirror_paused();
    r.store(1000); // full newer publish while the older one is parked
    r.release_mirror_gate();
    publisher.join().unwrap();
    for n in 0..4 {
        assert_eq!(**r.load_node(NodeId(n)), 1000, "node {n} regressed");
    }
}

#[test]
fn concurrent_rcu_all_replicas_converge_rv_numa_c54e6a13() {
    // Phase 1: real multi-threaded mirror coverage — 8x1000 rcu increments.
    let topo: Arc<dyn Topology> = Arc::new(MockTopology::with_nodes(4, 2));
    let r = Arc::new(NodeReplicated::new(topo, 0u64));
    let handles: Vec<_> = (0..8usize)
        .map(|_| {
            let r = Arc::clone(&r);
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    r.rcu(|v| v + 1);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    // Phase 1 result: all increments are applied exactly once and every
    // replica converged to the final value.
    for n in 0..4 {
        assert_eq!(**r.load_node(NodeId(n)), 8000, "node {n} after phase 1");
    }
    // Phase 2: deterministic delayed-mirror oracle (see the first test): the
    // parked mirror holds a stale capture (8001); unfixed code leaves it on
    // nodes 1..3 after the newer publish, fixed code re-reads node 0.
    r.arm_mirror_gate();
    let publisher = {
        let r = Arc::clone(&r);
        std::thread::spawn(move || {
            r.rcu(|v| v + 1); // node 0 = 8001, then pauses pre-mirror
        })
    };
    r.wait_mirror_paused();
    r.store(100_000);
    r.release_mirror_gate();
    publisher.join().unwrap();
    for n in 0..4 {
        assert_eq!(**r.load_node(NodeId(n)), 100_000);
    }
}
