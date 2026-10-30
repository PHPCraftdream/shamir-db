//! Declarative macros for concise query building.

#[macro_use]
mod bind;
#[macro_use]
mod doc;
#[macro_use]
mod subscribe;
#[macro_use]
mod vals;

#[cfg(test)]
mod tests;
