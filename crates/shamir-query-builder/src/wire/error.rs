/// Crate-owned codec failure, preserving whether encoding or decoding failed.
pub enum WireError {
    Encode(rmp_serde::encode::Error),
    Decode(rmp_serde::decode::Error),
}

impl std::fmt::Debug for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(_) => f.write_str("WireError::Encode(..)"),
            Self::Decode(_) => f.write_str("WireError::Decode(..)"),
        }
    }
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(error) => write!(f, "MessagePack encode failed: {error}"),
            Self::Decode(error) => write!(f, "MessagePack decode failed: {error}"),
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(error) => Some(error),
            Self::Decode(error) => Some(error),
        }
    }
}
