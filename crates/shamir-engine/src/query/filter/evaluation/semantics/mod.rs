//! Filter comparison semantics: value comparison, numeric cross-type
//! comparators, FTS matching, LIKE/regex pattern guarding and filter-value
//! resolution.

pub mod fts;
pub mod numeric_cmp;
pub mod pattern_guard;
pub mod resolve;

#[cfg(test)]
mod tests;
