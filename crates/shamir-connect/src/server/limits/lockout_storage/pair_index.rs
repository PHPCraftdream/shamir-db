use super::{PairKey, PairSlot, Subnet};

#[cfg(test)]
std::thread_local! {
    static RANK_PROBES: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
fn reset_rank_probes() {
    RANK_PROBES.with(|probes| probes.set(0));
}

#[cfg(test)]
fn rank_probe_count() -> usize {
    RANK_PROBES.with(core::cell::Cell::get)
}

#[cfg(test)]
#[path = "pair_index_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pair_collision_tests.rs"]
mod collision_tests;

// Each rank stores a physical payload slot, not a key hash. Used entries are sorted
// by payload key; `u16::MAX` sentinels occupy the unused suffix.
pub(super) fn key_cmp(left: PairKey, right: PairKey) -> core::cmp::Ordering {
    subnet_cmp(left.0, right.0).then_with(|| left.1.cmp(&right.1))
}

fn subnet_cmp(left: Subnet, right: Subnet) -> core::cmp::Ordering {
    match (left, right) {
        (Subnet::V4(a), Subnet::V4(b)) => a.cmp(&b),
        (Subnet::V4(_), Subnet::V6(_)) => core::cmp::Ordering::Less,
        (Subnet::V6(_), Subnet::V4(_)) => core::cmp::Ordering::Greater,
        (Subnet::V6(a), Subnet::V6(b)) => a.cmp(&b),
    }
}

fn used_len(slots: &[PairSlot]) -> usize {
    let mut low = 0;
    let mut high = slots.len();
    while low < high {
        let middle = low + (high - low) / 2;
        if slots[middle].index() == u16::MAX {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}

fn rank_for_key(slots: &[PairSlot], key: PairKey) -> Result<(usize, usize), usize> {
    let mut low = 0;
    let mut high = slots.len();
    while low < high {
        #[cfg(test)]
        RANK_PROBES.with(|probes| probes.set(probes.get() + 1));
        let middle = low + (high - low) / 2;
        let index = slots[middle].index();
        if index == u16::MAX {
            high = middle;
        } else {
            let physical = usize::from(index);
            match key_cmp(slots[physical].key, key) {
                core::cmp::Ordering::Less => low = middle + 1,
                core::cmp::Ordering::Greater => high = middle,
                core::cmp::Ordering::Equal => return Ok((middle, physical)),
            }
        }
    }
    Err(low)
}

pub(super) fn lookup(slots: &[PairSlot], key: PairKey) -> Option<usize> {
    rank_for_key(slots, key).ok().map(|(_, physical)| physical)
}

// Preconditions: `physical` contains a live payload whose key is not indexed.
pub(super) fn insert(slots: &mut [PairSlot], physical: usize) {
    let key = slots[physical].key;
    let insertion_rank = match rank_for_key(slots, key) {
        Ok(_) => panic!("pair key already indexed"),
        Err(rank) => rank,
    };
    let n = used_len(slots);
    assert!(n < slots.len());
    for rank in (insertion_rank..n).rev() {
        let index = slots[rank].index();
        slots[rank + 1].set_index(index);
    }
    slots[insertion_rank].set_index(u16::try_from(physical).expect("pair slot index exceeds u16"));
}

// Preconditions: `physical` is indexed. Removing its rank does not move payloads.
pub(super) fn remove(slots: &mut [PairSlot], physical: usize) {
    let key = slots[physical].key;
    let rank = rank_for_key(slots, key)
        .ok()
        .filter(|(_, indexed_physical)| *indexed_physical == physical)
        .map(|(rank, _)| rank)
        .expect("pair key is not indexed");
    let n = used_len(slots);
    for index_rank in rank..n - 1 {
        let index = slots[index_rank + 1].index();
        slots[index_rank].set_index(index);
    }
    slots[n - 1].set_index(u16::MAX);
}
