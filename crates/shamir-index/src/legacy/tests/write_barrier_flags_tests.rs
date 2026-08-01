use crate::legacy::write_barrier_flags::*;

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
        UNIQUE_INDEX_EXISTS | INDEX2_CREATE | SORTED_INDEX_CREATE
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
    let w = WriteBarrierFlags::with_unique_index_exists(true);
    assert!(w.is_set(UNIQUE_INDEX_EXISTS));
    assert!(w.any_set());

    let w2 = WriteBarrierFlags::with_unique_index_exists(false);
    assert!(!w2.any_set());
}

#[test]
fn clone_shares_the_same_underlying_word() {
    let a = WriteBarrierFlags::new();
    let b = a.clone();
    a.set(UNIQUE_INDEX_EXISTS);
    assert!(
        b.is_set(UNIQUE_INDEX_EXISTS),
        "clone must observe the same Arc<AtomicU8>"
    );
}

#[test]
fn all_six_bits_fit_in_one_byte_with_room_to_spare() {
    // Sanity check on the bit-packing budget claim in the module doc:
    // 6 bits used out of 8 available in AtomicU8, no overlap (if any pair
    // of the six constants aliased the same bit, `count_ones()` would be
    // less than 6 despite OR-ing all six together).
    assert_eq!(ALL_BITS.count_ones(), 6);
    assert_eq!(ALL_BITS, 0b0011_1111);
}
