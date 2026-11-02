use super::{insert, lookup, rank_probe_count, remove, reset_rank_probes, PairSlot};
use crate::server::lockout::{FailureState, PairKey, Subnet};
use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};

const CAPACITY: usize = 8_192;
const KEY_COUNT: usize = 32;
const MAX_CANDIDATE: u64 = 8_192 * 31 + 1;
const MAX_PROBES: usize = 14;

fn hash_slot(pair: &PairKey) -> usize {
    let mut hasher = FxHasher::default();
    pair.hash(&mut hasher);
    hasher.finish() as usize % CAPACITY
}

fn candidate_pair(candidate: u64) -> PairKey {
    let mut user = [0u8; 16];
    user[..8].copy_from_slice(&candidate.to_be_bytes());
    (Subnet::V4([192, 0, 2]), user)
}

fn colliding_pairs() -> (usize, Vec<PairKey>) {
    let mut counts = vec![0u8; CAPACITY];
    let mut target = None;
    for candidate in 0..MAX_CANDIDATE {
        let bucket = hash_slot(&candidate_pair(candidate));
        counts[bucket] += 1;
        if usize::from(counts[bucket]) == KEY_COUNT {
            target = Some(bucket);
            break;
        }
    }
    let target = target.expect("pigeonhole bound guarantees 32 colliding keys");

    let mut pairs = Vec::with_capacity(KEY_COUNT);
    for candidate in 0..MAX_CANDIDATE {
        let pair = candidate_pair(candidate);
        if hash_slot(&pair) == target {
            pairs.push(pair);
            if pairs.len() == KEY_COUNT {
                break;
            }
        }
    }
    assert_eq!(pairs.len(), KEY_COUNT);
    (target, pairs)
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
fn hash_collisions_keep_pair_index_lookups_bounded_through_remove_and_reinsert() {
    let (target, pairs) = colliding_pairs();
    let mut slots = vec![PairSlot::default(); CAPACITY];

    for (i, pair) in pairs.iter().copied().enumerate() {
        assert_eq!(hash_slot(&pair), target);
        let physical = (i * 4_051) % CAPACITY;
        slots[physical].replace_payload(
            pair,
            FailureState {
                count: 1,
                last_fail_at_ns: 7,
            },
            None,
        );
        insert(&mut slots, physical);
    }

    for (i, pair) in pairs.iter().copied().enumerate() {
        checked_lookup(&slots, pair, Some((i * 4_051) % CAPACITY));
    }

    let first_physical = 0;
    remove(&mut slots, first_physical);
    slots[first_physical].clear_payload();
    checked_lookup(&slots, pairs[0], None);
    for (i, pair) in pairs.iter().copied().enumerate().skip(1) {
        checked_lookup(&slots, pair, Some((i * 4_051) % CAPACITY));
    }

    slots[first_physical].replace_payload(
        pairs[0],
        FailureState {
            count: 2,
            last_fail_at_ns: 9,
        },
        None,
    );
    insert(&mut slots, first_physical);
    for (i, pair) in pairs.iter().copied().enumerate() {
        checked_lookup(&slots, pair, Some((i * 4_051) % CAPACITY));
    }
}
