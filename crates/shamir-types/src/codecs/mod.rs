pub mod basic;
mod codec;
mod error;
pub mod interned;
#[cfg(test)]
pub mod tests;

pub use codec::Codec;
pub use error::CodecError;

// Re-export basic codecs for convenience
pub use basic::{from_bytes, to_bytes, JsonCodec, MessagePackCodec};
