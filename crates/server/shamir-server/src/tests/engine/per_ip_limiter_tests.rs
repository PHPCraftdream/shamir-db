use std::net::{IpAddr, Ipv4Addr};

use crate::conn_limiter::PerIpLimiter;

#[test]
fn per_ip_cap_zero_is_unlimited() {
    let l = PerIpLimiter::new(0);
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let _g1 = l.try_acquire(ip).expect("first acquire");
    let _g2 = l.try_acquire(ip).expect("second acquire");
    let _g3 = l.try_acquire(ip).expect("third acquire");
    // No cap configured → always Some, no map entries.
    assert_eq!(l.active(ip), 0);
}

#[test]
fn per_ip_cap_allows_n_then_rejects_same_ip() {
    let l = PerIpLimiter::new(3);
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let g1 = l.try_acquire(ip).expect("1");
    let g2 = l.try_acquire(ip).expect("2");
    let _g3 = l.try_acquire(ip).expect("3");
    assert_eq!(l.active(ip), 3, "three active from the same IP");
    assert!(
        l.try_acquire(ip).is_none(),
        "4th from the same IP must be rejected"
    );

    // Drop one — next acquire from the same IP succeeds.
    drop(g1);
    assert_eq!(l.active(ip), 2);
    let _g4 = l.try_acquire(ip).expect("after release");
    assert_eq!(l.active(ip), 3);
    let _ = (g2,);
}

#[test]
fn per_ip_cap_independent_across_ips() {
    let l = PerIpLimiter::new(2);
    let ip_a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    let ip_b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

    // Fill ip_a to its cap.
    let _a1 = l.try_acquire(ip_a).expect("a1");
    let _a2 = l.try_acquire(ip_a).expect("a2");
    assert!(l.try_acquire(ip_a).is_none(), "ip_a at cap");

    // ip_b is independent — still allowed up to ITS cap.
    let _b1 = l.try_acquire(ip_b).expect("b1");
    let _b2 = l.try_acquire(ip_b).expect("b2");
    assert!(l.try_acquire(ip_b).is_none(), "ip_b at cap");

    // ip_a still blocked while ip_b holds its slots.
    assert!(
        l.try_acquire(ip_a).is_none(),
        "ip_a still at cap even though ip_b is full"
    );
}

#[test]
fn per_ip_drop_releases_slot() {
    let l = PerIpLimiter::new(1);
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    {
        let _g = l.try_acquire(ip).expect("first");
        assert_eq!(l.active(ip), 1);
        assert!(l.try_acquire(ip).is_none(), "cap reached");
    }
    // Out of scope — slot freed, and the zero-count entry is pruned.
    assert_eq!(l.active(ip), 0, "slot freed after drop");
    // Map should not grow unboundedly: a new acquire after prune still works.
    let _g = l.try_acquire(ip).expect("after drop");
}

#[test]
fn per_ip_zero_count_entry_pruned_on_release() {
    // The map must not accumulate stale zero-count entries for every
    // historical IP — Drop prunes entries that fall back to 0.
    let l = PerIpLimiter::new(5);
    let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
    {
        let _g = l.try_acquire(ip).expect("acquire");
        assert_eq!(l.active(ip), 1);
    }
    // After the guard drops, the entry should be gone (count == 0 → prune).
    assert_eq!(l.active(ip), 0);
    // distinct_entries should be 0 if we exposed it; we infer via active().
    // A fresh acquire re-creates the entry.
    let _g2 = l.try_acquire(ip).expect("re-acquire after prune");
    assert_eq!(l.active(ip), 1);
}

#[test]
fn per_ip_clones_share_state() {
    let l1 = PerIpLimiter::new(2);
    let l2 = l1.clone();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let _g1 = l1.try_acquire(ip).expect("from l1");
    assert_eq!(l2.active(ip), 1, "l2 sees l1's acquire");
    let _g2 = l2.try_acquire(ip).expect("from l2");
    assert!(
        l1.try_acquire(ip).is_none(),
        "l1 sees l2's acquire — cap reached"
    );
}
