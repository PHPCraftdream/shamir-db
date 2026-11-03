use std::net::IpAddr;
use std::sync::Arc;

use super::super::browser::PreAuthAdmission;

fn ip(value: &str) -> IpAddr {
    value.parse().unwrap()
}

#[test]
fn subnet_limit_is_eight_and_permits_release_to_zero() {
    let admission = Arc::new(PreAuthAdmission::default());
    let peer = ip("10.1.2.3");
    let permits = (0..8)
        .map(|_| admission.try_acquire(peer).expect("within subnet limit"))
        .collect::<Vec<_>>();
    assert!(admission.try_acquire(peer).is_none());
    assert_eq!(admission.in_use(peer), (8, 8));
    drop(permits);
    assert_eq!(admission.in_use(peer), (0, 0));
}

#[test]
fn global_limit_is_256_across_distinct_subnets() {
    let admission = Arc::new(PreAuthAdmission::default());
    let permits = (0_u32..256)
        .map(|index| {
            let address = IpAddr::V4(std::net::Ipv4Addr::from(index << 8));
            admission.try_acquire(address).expect("within global limit")
        })
        .collect::<Vec<_>>();
    assert!(admission.try_acquire(ip("192.0.2.1")).is_none());
    drop(permits);
    assert_eq!(admission.in_use(ip("192.0.2.1")).0, 0);
}

#[test]
fn concurrent_last_drop_and_registration_preserves_subnet_cap() {
    let peer = ip("198.51.100.4");

    for _ in 0..64 {
        let admission = Arc::new(PreAuthAdmission::default());
        let old_permit = admission.try_acquire(peer).expect("acquire old permit");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let worker_admission = Arc::clone(&admission);
        let worker_barrier = Arc::clone(&barrier);
        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            worker_admission.try_acquire(peer)
        });
        barrier.wait();
        drop(old_permit);
        let new_permit = worker.join().unwrap().expect("acquire after last drop");

        let mut permits = vec![new_permit];
        permits.extend((0..7).map(|_| admission.try_acquire(peer).expect("fill subnet")));
        assert_eq!(admission.in_use(peer), (8, 8));
        assert!(admission.try_acquire(peer).is_none());
        drop(permits);
        assert_eq!(admission.in_use(peer), (0, 0));
    }
}
