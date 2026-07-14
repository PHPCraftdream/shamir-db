//! Test manifest for `key_bytes`. Topic-split files, one per the
//! seven mandatory test categories in
//! `docs/dev-artifacts/design/record-key-128-migration-plan.md` step 1.

pub mod boundary_tests;
pub mod conversions_tests;
pub mod debug_tests;
pub mod eq_ord_tests;
pub mod hash_consistency_tests;
pub mod serde_byte_identity_tests;
pub mod size_tests;
