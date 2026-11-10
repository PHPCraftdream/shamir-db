//! The guest-module ABI contract shared by admission and the runtime.

/// Name of the required linear-memory export.
pub const EXPORT_MEMORY: &str = "memory";
/// Name of the required allocator export (`(i32) -> i32`).
pub const EXPORT_ALLOC: &str = "shamir_alloc";
/// Name of the required entrypoint export (`(i32, i32) -> i64`).
pub const EXPORT_CALL: &str = "shamir_call";
/// Checked stored-call import name.
pub const IMPORT_CALL_CHECKED: &str = "call_checked";
/// Name of the ABI-version custom section.
pub const ABI_SECTION: &str = "shamir.abi";
/// Name of the origin-claim custom section.
pub const ORIGIN_SECTION: &str = "shamir.origin";
/// The one ABI version this build accepts (the default when the section is absent).
pub const SUPPORTED_ABI_VERSION: u32 = 1;
/// Longest accepted `shamir.origin` payload in bytes.
pub const MAX_ORIGIN_BYTES: usize = 64;
