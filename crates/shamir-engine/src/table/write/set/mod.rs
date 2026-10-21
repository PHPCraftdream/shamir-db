//! SET (upsert) execution (`execute_set_tx`) on `TableManager`.
#[allow(clippy::module_inception)]
// the implementation file has the same name as its module (manifest routing)
pub(in crate::table::write) mod set;
