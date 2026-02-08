//! Table module - CRUD operations with interning

pub mod counter;
pub mod interner;
pub mod table;

#[cfg(test)]
pub mod tests;

pub use table::Table;
