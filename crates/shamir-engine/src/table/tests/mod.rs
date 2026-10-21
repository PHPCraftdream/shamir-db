pub(crate) mod admission;
pub(crate) mod behavior;
pub(crate) mod index_lifecycle;
pub(crate) mod read;
pub(crate) mod table_core;
pub(crate) mod write;

// Stable pre-sweep paths: re-exports keep `crate::table::tests::<old name>` working.
#[cfg(feature = "fjall")]
pub(crate) use behavior::schema::test_helpers;
#[cfg(feature = "fjall")]
#[cfg(feature = "fjall")]
pub(crate) use read::streaming::stream_utils;
pub(crate) use write::record_mutation::execution::fixtures::write_exec_tests;
