pub mod basic;
mod codec;
mod error;
pub mod interned;
pub mod legacy;
#[cfg(test)]
pub mod tests;

pub use codec::Codec;
pub use error::CodecError;
pub use legacy::tools as transform;

// Re-export basic codecs for convenience
pub use basic::{from_bytes, to_bytes, JsonCodec, MessagePackCodec};
