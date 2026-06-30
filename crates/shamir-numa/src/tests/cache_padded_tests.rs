//! `CachePadded` — alignment + transparency.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::CachePadded;

#[test]
fn aligned_to_at_least_128_bytes() {
    assert!(std::mem::align_of::<CachePadded<u8>>() >= 128);
    assert!(std::mem::align_of::<CachePadded<AtomicU64>>() >= 128);
}

#[test]
fn size_is_padded_to_the_alignment() {
    // A single byte still occupies a full cache-line slot — that is the whole
    // point: two adjacent CachePadded values cannot share a line.
    assert!(std::mem::size_of::<CachePadded<u8>>() >= 128);
}

#[test]
fn adjacent_values_do_not_share_a_line() {
    // Two CachePadded cells laid out in an array must be ≥128 bytes apart.
    let arr = [CachePadded::new(0u8), CachePadded::new(0u8)];
    let a = std::ptr::addr_of!(arr[0]) as usize;
    let b = std::ptr::addr_of!(arr[1]) as usize;
    assert!(b - a >= 128, "adjacent cells only {} bytes apart", b - a);
}

#[test]
fn deref_is_transparent() {
    let c = CachePadded::new(AtomicU64::new(41));
    c.fetch_add(1, Ordering::Relaxed); // via Deref
    assert_eq!(c.load(Ordering::Relaxed), 42);
    assert_eq!(CachePadded::new(7u32).into_inner(), 7);
}

#[test]
fn deref_mut_is_transparent() {
    let mut c = CachePadded::new(10u32);
    *c += 5;
    assert_eq!(*c, 15);
}
