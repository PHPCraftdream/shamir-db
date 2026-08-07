//! Vector distance metric types.
//!
//! Defines the distance/similarity metrics for vector indexes.

/// Vector distance metric.
///
/// Determines how vector similarity is computed for approximate nearest neighbor search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Euclidean (L2) distance.
    L2,
    /// Cosine similarity.
    Cosine,
    /// Dot product.
    Dot,
}

impl Metric {
    /// Returns the wire representation of this metric.
    pub fn as_str(self) -> &'static str {
        match self {
            Metric::L2 => "l2",
            Metric::Cosine => "cosine",
            Metric::Dot => "dot",
        }
    }
}

impl From<Metric> for String {
    fn from(metric: Metric) -> Self {
        metric.as_str().to_string()
    }
}
