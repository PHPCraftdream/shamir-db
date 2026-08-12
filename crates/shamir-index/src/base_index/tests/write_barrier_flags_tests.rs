use crate::base_index::write_barrier_flags::*;

#[test]
fn new_word_has_no_bits_set() {
    let w = WriteBarrierFlags::new();
    assert!(!w.any_set());
    assert_eq!(w.raw(), 0);
}

#[test]
fn set_and_clear_a_single_bit() {
    let w = WriteBarrierFlags::new();
    w.set(SCHEMA_ACTIVATION);
    assert!(w.any_set());
    assert!(w.is_set(SCHEMA_ACTIVATION));
    assert!(!w.is_set(UNIQUE_INDEX_EXISTS));
    w.clear(SCHEMA_ACTIVATION);
    assert!(!w.any_set());
}

#[test]
fn independent_bits_do_not_clobber_each_other() {
    let w = WriteBarrierFlags::new();
    w.set(UNIQUE_INDEX_EXISTS);
    w.set(INDEX2_CREATE);
    w.set(SORTED_INDEX_CREATE);
    assert_eq!(
        w.raw(),
        (UNIQUE_INDEX_EXISTS | INDEX2_CREATE | SORTED_INDEX_CREATE) as u16
    );

    // Clearing one bit must not disturb the others.
    w.clear(INDEX2_CREATE);
    assert!(w.is_set(UNIQUE_INDEX_EXISTS));
    assert!(!w.is_set(INDEX2_CREATE));
    assert!(w.is_set(SORTED_INDEX_CREATE));
    assert!(w.any_set());
}

#[test]
fn set_to_toggles_both_directions() {
    let w = WriteBarrierFlags::new();
    w.set_to(REGULAR_INDEX_CREATE, true);
    assert!(w.is_set(REGULAR_INDEX_CREATE));
    w.set_to(REGULAR_INDEX_CREATE, false);
    assert!(!w.is_set(REGULAR_INDEX_CREATE));
}

#[test]
fn with_unique_index_exists_preseeds_bit_zero() {
    let w = WriteBarrierFlags::with_regular_and_unique_index_exists(false, true);
    assert!(w.is_set(UNIQUE_INDEX_EXISTS));
    assert!(w.any_set());

    let w2 = WriteBarrierFlags::with_regular_and_unique_index_exists(false, false);
    assert!(!w2.any_set());
}

#[test]
fn with_regular_and_unique_index_exists_preseeds_both_bits() {
    let w = WriteBarrierFlags::with_regular_and_unique_index_exists(true, true);
    assert!(w.is_set(REGULAR_INDEX_EXISTS));
    assert!(w.is_set(UNIQUE_INDEX_EXISTS));
    assert!(w.any_set());

    let w2 = WriteBarrierFlags::with_regular_and_unique_index_exists(false, true);
    assert!(!w2.is_set(REGULAR_INDEX_EXISTS));
    assert!(w2.is_set(UNIQUE_INDEX_EXISTS));

    let w3 = WriteBarrierFlags::with_regular_and_unique_index_exists(true, false);
    assert!(w3.is_set(REGULAR_INDEX_EXISTS));
    assert!(!w3.is_set(UNIQUE_INDEX_EXISTS));
    // REGULAR_INDEX_EXISTS is excluded from BARRIER_BITS, so any_set() is false
    assert!(!w3.any_set());

    let w4 = WriteBarrierFlags::with_regular_and_unique_index_exists(false, false);
    assert!(!w4.is_set(REGULAR_INDEX_EXISTS));
    assert!(!w4.is_set(UNIQUE_INDEX_EXISTS));
    assert!(!w4.any_set());
}

#[test]
fn clone_shares_the_same_underlying_word() {
    let a = WriteBarrierFlags::new();
    let b = a.clone();
    a.set(UNIQUE_INDEX_EXISTS);
    assert!(
        b.is_set(UNIQUE_INDEX_EXISTS),
        "clone must observe the same Arc<AtomicU16>"
    );
}

#[test]
fn all_seven_bits_fit_in_two_bytes_with_room_to_spare() {
    // Sanity check on the bit-packing budget claim in the module doc:
    // 7 bits used out of 16 available in AtomicU16, no overlap (if any pair
    // of the seven constants aliased the same bit, `count_ones()` would be
    // less than 7 despite OR-ing all seven together).
    assert_eq!(ALL_BITS.count_ones(), 7);
    assert_eq!(ALL_BITS, 0b0100_0000 | 0b0011_1111);
}
