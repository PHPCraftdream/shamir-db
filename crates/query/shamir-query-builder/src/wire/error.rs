/// Crate-owned codec failure, preserving whether encoding or decoding failed.
#[derive(Debug)]
pub enum WireError {
    Encode(String),
    Decode(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(message) => write!(f, "MessagePack encode failed: {message}"),
            Self::Decode(message) => write!(f, "MessagePack decode failed: {message}"),
        }
    }
}

impl std::error::Error for WireError {}
