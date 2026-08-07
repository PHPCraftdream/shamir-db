//! Vector quantization types.
//!
//! Defines the quantization options for vector indexes.

/// Vector quantization mode.
///
/// Determines how vector values are quantized for storage and search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantization {
    /// No quantization: store and search with full f32 precision.
    Off,
    /// SQ8 scalar quantization: quantize to 8-bit integers.
    ///
    /// V5.2 #411 — deferred fit at 256 vectors → u8-code HNSW graph with dequant-rescore.
    Sq8,
}

impl Quantization {
    /// Returns the wire representation of this quantization mode.
    ///
    /// Returns `None` for `Off` (no quantization field in wire format).
    pub fn as_str(self) -> Option<&'static str> {
        match self {
            Quantization::Off => None,
            Quantization::Sq8 => Some("sq8"),
        }
    }
}

impl From<Quantization> for Option<String> {
    fn from(quantization: Quantization) -> Self {
        quantization.as_str().map(str::to_string)
    }
}
