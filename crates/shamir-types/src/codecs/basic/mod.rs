pub mod bincode;
pub mod messagepack;

pub use bincode::{from_bytes, to_bytes};
pub use messagepack::MessagePackCodec;

#[cfg(test)]
pub mod tests;
