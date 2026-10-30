use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, OnceLock};

use crate::platform::detect::{cached_with, detect_with};
use crate::{AffinityError, FallbackSingleNodeTopology};

#[test]
fn cached_probe_initializes_once_concurrently_and_shares_identity() {
    let cache = OnceLock::new();
    let diagnostic = OnceLock::new();
    let calls = AtomicUsize::new(0);
    let reports = AtomicUsize::new(0);
    let barrier = Barrier::new(8);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    cached_with(&cache, || {
                        detect_with(
                            || {
                                calls.fetch_add(1, Ordering::SeqCst);
                                Ok(Arc::new(FallbackSingleNodeTopology::with_cpus(2)))
                            },
                            &diagnostic,
                            |_| {
                                reports.fetch_add(1, Ordering::SeqCst);
                            },
                        )
                    })
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(results.iter().all(|value| Arc::ptr_eq(value, &results[0])));
        assert_eq!(results[0].cores_on_node(crate::NodeId(0)).len(), 2);
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(reports.load(Ordering::SeqCst), 0);
}

#[test]
fn concurrent_failed_initialization_reports_once_and_caches_fallback() {
    let cache = OnceLock::new();
    let diagnostic = OnceLock::new();
    let calls = AtomicUsize::new(0);
    let reports = AtomicUsize::new(0);
    let barrier = Barrier::new(8);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    cached_with(&cache, || {
                        detect_with(
                            || {
                                calls.fetch_add(1, Ordering::SeqCst);
                                Err(AffinityError::Unsupported)
                            },
                            &diagnostic,
                            |error| {
                                assert!(matches!(error, AffinityError::Unsupported));
                                reports.fetch_add(1, Ordering::SeqCst);
                            },
                        )
                    })
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(results.iter().all(|value| Arc::ptr_eq(value, &results[0])));
        assert_eq!(results[0].num_nodes(), 1);
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(reports.load(Ordering::SeqCst), 1);
}

#[test]
fn cached_failure_and_fresh_probes_share_one_classified_diagnostic() {
    let cache = OnceLock::new();
    let diagnostic = OnceLock::new();
    let calls = AtomicUsize::new(0);
    let reports = AtomicUsize::new(0);
    let probe = || {
        detect_with(
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(AffinityError::Syscall(io::Error::from(
                    io::ErrorKind::PermissionDenied,
                )))
            },
            &diagnostic,
            |error| {
                assert!(matches!(
                    error,
                    AffinityError::Syscall(e) if e.kind() == io::ErrorKind::PermissionDenied
                ));
                reports.fetch_add(1, Ordering::SeqCst);
            },
        )
    };
    let first = cached_with(&cache, probe);
    let second = cached_with(&cache, probe);
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.num_nodes(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let fresh_a = probe();
    let fresh_b = probe();
    assert!(!Arc::ptr_eq(&first, &fresh_a));
    assert!(!Arc::ptr_eq(&fresh_a, &fresh_b));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(reports.load(Ordering::SeqCst), 1);
}

#[test]
fn public_cached_topology_has_shared_identity() {
    assert!(Arc::ptr_eq(
        &crate::detect_cached(),
        &crate::detect_cached()
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn off_linux_detect_returns_fresh_single_node_fallback() {
    let first = crate::detect();
    let second = crate::detect();
    assert_eq!(first.num_nodes(), 1);
    assert_eq!(first.current_node(), crate::NodeId(0));
    assert!(!first.cores_on_node(crate::NodeId(0)).is_empty());
    assert!(!Arc::ptr_eq(&first, &second));
}
