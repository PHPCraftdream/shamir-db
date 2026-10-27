//! Serialize builder-produced wire DTOs to transport formats.

mod error;
mod to_wire;

pub mod response;

pub use error::WireError;
pub use to_wire::ToWire;

#[cfg(test)]
mod tests;
