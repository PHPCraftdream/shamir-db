use thiserror::Error;

/// Maximum nesting depth. Mirrors `messagepack::MAX_MSGPACK_DEPTH` so the lens
/// and the tree decoder agree on where "too deep" begins; deeper input returns
/// [`RecordViewError::DepthExceeded`] rather than overflowing the stack.
pub const MAX_MSGPACK_DEPTH: usize = 128;

/// Errors raised by the lens on malformed/truncated/unexpected input. The lens
/// is untrusted-input safe: it surfaces one of these (or `None` for a
/// single-field miss) and never panics.
#[derive(Error, Debug)]
pub enum RecordViewError {
    /// The buffer ended before a full marker+payload could be read.
    #[error("truncated MessagePack buffer at byte {0}")]
    Truncated(usize),
    /// The top-level marker (or a context requiring a map) was not a map.
    #[error("non-map marker where a map was expected (byte {got:#04x})")]
    NonMapTopLevel { got: u8 },
    /// A string payload was not valid UTF-8 (the canonical decoder rejects the
    /// same; the lens only validates when a `&str` is materialised).
    #[error("invalid UTF-8 in string at byte {0}")]
    InvalidUtf8(usize),
    /// A map key was not a `bin` marker (the storage codec encodes
    /// `InternerKey` as msgpack `bin`; encountering a non-bin key means the
    /// buffer is not in storage form).
    #[error("non-bin map key at byte {0}")]
    NonBinKey(usize),
    /// Nesting deeper than [`MAX_MSGPACK_DEPTH`].
    #[error("MessagePack nesting depth exceeds {0}")]
    DepthExceeded(usize),
    /// A reserved marker byte (`0xc1`) was encountered — the canonical decoder
    /// rejects these too.
    #[error("reserved MessagePack marker at byte {0}")]
    ReservedMarker(usize),
    /// An unknown marker byte outside the MessagePack spec.
    #[error("unknown MessagePack marker {got:#04x} at byte {at}")]
    UnknownMarker { got: u8, at: usize },
}

impl RecordViewError {
    /// Lift the canonical decoder's depth-cap constant into the error text —
    /// kept here so the two stay numerically identical without a cross-module
    /// const import (the decoder's is private by design).
    #[inline]
    pub(super) fn depth_cap() -> usize {
        MAX_MSGPACK_DEPTH
    }
}
