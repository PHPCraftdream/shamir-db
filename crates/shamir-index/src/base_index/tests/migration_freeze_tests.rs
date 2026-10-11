use crate::base_index::write_barrier_flags::{
    WriteBarrierFlags, ALL_BITS, BARRIER_BITS, MIGRATION_FREEZE, REGULAR_INDEX_EXISTS,
    SCHEMA_ACTIVATION,
};

/// Omitting the freeze from the predicate leaves a regular-index-only table
/// on the fast path; omitting it from ALL_BITS loses the packed-word contract.
#[test]
fn migration_freeze_forces_barrier_without_changing_regular_index_policy() {
    let flags = WriteBarrierFlags::new();
    flags.set(REGULAR_INDEX_EXISTS);
    assert!(!flags.any_set());
    flags.set(MIGRATION_FREEZE);
    assert!(flags.any_set());
    assert_ne!(BARRIER_BITS & u16::from(MIGRATION_FREEZE), 0);
    assert_ne!(ALL_BITS & u16::from(MIGRATION_FREEZE), 0);
    flags.clear(MIGRATION_FREEZE);
    assert!(!flags.any_set());
    assert!(flags.is_set(REGULAR_INDEX_EXISTS));
}

/// Returning the old bit's presence inverts ownership; replacing the word
/// rather than fetch_or loses another barrier owner's bit.
#[test]
fn try_set_reports_new_owner_and_preserves_other_bits_across_clones() {
    let flags = WriteBarrierFlags::new();
    let clone = flags.clone();
    flags.set(SCHEMA_ACTIVATION);
    assert!(flags.try_set(MIGRATION_FREEZE));
    assert!(!clone.try_set(MIGRATION_FREEZE));
    assert_eq!(flags.raw(), u16::from(SCHEMA_ACTIVATION | MIGRATION_FREEZE),);
    clone.clear(MIGRATION_FREEZE);
    assert!(flags.try_set(MIGRATION_FREEZE));
    flags.clear(MIGRATION_FREEZE);
    assert_eq!(clone.raw(), u16::from(SCHEMA_ACTIVATION));
}
