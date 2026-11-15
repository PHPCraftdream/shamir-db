#[cfg(test)]
pub mod tests;

#[allow(clippy::module_inception)]
pub mod db_instance;

pub use crate::table::table_manager::migration;
