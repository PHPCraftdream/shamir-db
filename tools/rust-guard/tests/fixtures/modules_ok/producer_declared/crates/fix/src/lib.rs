//! A macro whose nested body declares a module, with BOTH the producer
//! and the generated file properly declared. The producer name is
//! `macro_rules`: the `macro_rules!` DEFINITION is itself an item macro
//! whose path segment is `macro_rules`, and its tokens (after the
//! descend-into-groups fix) mention `mod`.
//!
//! The `[[producer]]` proves the `[[producer]]` spelling is accepted by
//! the loader (the serde field was `producers` with no rename — the
//! documented spelling silently deserialized to an empty list, so the
//! macro went UNRESOLVED_MACRO and the run failed).

macro_rules! declare_mods {
    () => {
        mod generated;
    };
}
declare_mods!();

pub fn existing() -> u32 {
    1
}
