//! Lifecycle and permission behavior of the wire cursor handler: cursor
//! create/reap/cancel lifecycle and permission checks, plus index-seek
//! driven cursor walks. Declarations/re-exports only; fixtures live in
//! `super::common`.

pub mod index_seek_tests;
pub mod lifecycle_permission_tests;
