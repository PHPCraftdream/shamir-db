use super::{
    compact, insert, key_cmp, lookup, rank_probe_count, remove, reset_rank_probes, used_len,
    PairSlot,
};
use crate::server::lockout::{
    FailureOutcome, FailureState, InMemoryLockoutStore, LockoutStore, PairKey, Subnet,
    BACKOFF_RESET_NS,
};

const CAPACITY: usize = 8_192;
const MAX_PROBES: usize = 14;

fn key(i: usize) -> PairKey {
    let mut user = [0u8; 16];
    user[..8].copy_from_slice(&(i as u64).to_be_bytes());
    user[15] = (i as u8).wrapping_mul(37);
    if i.is_multiple_of(2) {
        (Subnet::V4([10, (i >> 8) as u8, i as u8]), user)
    } else {
        let mut prefix = [0u8; 8];
        prefix[..4].copy_from_slice(&(i as u32).to_be_bytes());
        (Subnet::V6(prefix), user)
    }
}

fn slots() -> Vec<PairSlot> {
    vec![PairSlot::default(); CAPACITY]
}

fn add(slots: &mut [PairSlot], physical: usize, pair: PairKey) {
    slots[physical].replace_payload(
        pair,
        FailureState {
            count: 1,
            last_fail_at_ns: 7,
        },
        None,
    );
    insert(slots, physical);
}

fn assert_bounded_public_lookup(probes: usize) {
    assert!(
        probes > 0 && probes <= MAX_PROBES,
        "unexpected rank probes: {probes}"
    );
}

#[test]
fn handshake_and_existing_pair_lookups_use_bounded_index_when_full() {
    let store = InMemoryLockoutStore::new();
    {
        let mut slots = store.slots.lock();
        for i in 0..CAPACITY {
            add(&mut slots, (i * 4_051) % CAPACITY, key(i));
        }
    }

    reset_rank_probes();
    assert!(!store.is_locked_out(key(0), 7));
    assert_bounded_public_lookup(rank_probe_count());

    reset_rank_probes();
    assert_eq!(store.current_backoff_ms(key(CAPACITY - 1), 7), 100);
    assert_bounded_public_lookup(rank_probe_count());

    reset_rank_probes();
    assert_eq!(
        store.register_failure(key(CAPACITY / 2), 7),
        FailureOutcome::Backoff { delay_ms: 200 }
    );
    assert_bounded_public_lookup(rank_probe_count());

    reset_rank_probes();
    assert!(!store.is_locked_out(key(CAPACITY + 3), 7));
    assert_bounded_public_lookup(rank_probe_count());
}

fn checked_lookup(slots: &[PairSlot], pair: PairKey, expected: Option<usize>) {
    reset_rank_probes();
    assert_eq!(lookup(slots, pair), expected);
    assert!(
        rank_probe_count() <= MAX_PROBES,
        "binary search exceeded probe bound"
    );
}

#[test]
fn lookups_are_bounded_and_preserve_physical_slots_at_all_fill_levels() {
    let mut slots = slots();
    checked_lookup(&slots, key(0), None);
    let partial = 1_024;
    for i in 0..partial {
        add(&mut slots, (i * 4_051) % CAPACITY, key(i));
    }
    for i in 0..partial {
        checked_lookup(&slots, key(i), Some((i * 4_051) % CAPACITY));
    }
    checked_lookup(&slots, key(CAPACITY + 3), None);
    for i in partial..CAPACITY {
        add(&mut slots, (i * 4_051) % CAPACITY, key(i));
    }
    assert_eq!(slots.len(), CAPACITY);
    for i in 0..CAPACITY {
        checked_lookup(&slots, key(i), Some((i * 4_051) % CAPACITY));
    }
    checked_lookup(&slots, key(CAPACITY + 3), None);
}

#[test]
fn removals_clears_and_replacements_keep_index_reachable() {
    let mut slots = slots();
    for i in 0..512 {
        add(&mut slots, i, key(i));
    }
    for i in (0..512).step_by(3) {
        let physical = i;
        remove(&mut slots, physical);
        slots[physical].clear_payload();
    }
    for i in 0..512_usize {
        let physical = i;
        if i.is_multiple_of(3) {
            checked_lookup(&slots, key(i), None);
            add(&mut slots, physical, key(CAPACITY + i));
            checked_lookup(&slots, key(CAPACITY + i), Some(physical));
        } else {
            checked_lookup(&slots, key(i), Some(physical));
        }
    }
}

#[test]
fn gc_removes_stale_payload_from_index_at_reset_boundary() {
    let store = InMemoryLockoutStore::new();
    let pair = key(0);
    assert_eq!(
        store.register_failure(pair, 7),
        FailureOutcome::Backoff { delay_ms: 100 }
    );
    {
        let slots = store.slots.lock();
        assert_eq!(slots.iter().filter(|slot| slot.occupied).count(), 1);
        assert!(lookup(&slots, pair).is_some());
    }

    store.gc(7 + BACKOFF_RESET_NS);

    {
        let slots = store.slots.lock();
        assert!(slots.iter().all(|slot| !slot.occupied));
        assert!(lookup(&slots, pair).is_none());
        assert_eq!(super::used_len(&slots), 0);
    }

    assert_eq!(
        store.register_failure(key(1), 7 + BACKOFF_RESET_NS),
        FailureOutcome::Backoff { delay_ms: 100 }
    );
}

#[test]
fn shared_prefix_keys_are_collision_independent_and_comparator_is_total() {
    let mut slots = slots();
    let mut pairs = Vec::with_capacity(256);
    for i in 0..256 {
        let mut user = [0x5a; 16];
        user[15] = i as u8;
        let pair = if i < 128 {
            (Subnet::V4([192, 0, 2]), user)
        } else {
            (Subnet::V6([0x20, 1, 0xdb, 8, 0, 0, 0, 1]), user)
        };
        add(&mut slots, (i * 4_051) % CAPACITY, pair);
        pairs.push(pair);
    }
    for (i, pair) in pairs.iter().copied().enumerate() {
        checked_lookup(&slots, pair, Some((i * 4_051) % CAPACITY));
    }
    assert!(key_cmp(pairs[0], pairs[1]).is_lt());
    assert!(key_cmp(pairs[127], pairs[128]).is_lt());
    assert_eq!(key_cmp(pairs[42], pairs[42]), core::cmp::Ordering::Equal);
}

#[test]
fn compact_after_bulk_clear_keeps_survivors_findable_in_linear_time() {
    let mut slots = slots();
    for i in 0..CAPACITY {
        add(&mut slots, (i * 4_051) % CAPACITY, key(i));
    }
    for i in (0..CAPACITY).step_by(2) {
        slots[(i * 4_051) % CAPACITY].clear_payload();
    }
    compact(&mut slots);
    assert_eq!(used_len(&slots), CAPACITY / 2);
    for i in 0..CAPACITY {
        let expected = (i % 2 == 1).then_some((i * 4_051) % CAPACITY);
        assert_eq!(lookup(&slots, key(i)), expected, "key {i}");
    }
    for i in (0..CAPACITY).step_by(2) {
        slots[(i * 4_051) % CAPACITY].clear_payload();
    }
    for i in (1..CAPACITY).step_by(2) {
        slots[(i * 4_051) % CAPACITY].clear_payload();
    }
    compact(&mut slots);
    assert_eq!(used_len(&slots), 0);
}
