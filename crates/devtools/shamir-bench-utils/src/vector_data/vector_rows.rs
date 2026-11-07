//! Contiguous row storage for generated vector datasets.

/// Contiguous rows with slice-like indexing and iteration.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorRows {
    data: Vec<f32>,
    dim: usize,
}

impl VectorRows {
    pub(super) fn with_capacity(rows: usize, dim: usize) -> Self {
        Self {
            data: Vec::with_capacity(rows.saturating_mul(dim)),
            dim,
        }
    }

    pub(super) fn push(&mut self, row: &[f32]) {
        // Append directly into flat storage; each append may grow its backing buffer.
        self.data.extend_from_slice(row);
    }

    /// Number of rows.
    pub fn len(&self) -> usize {
        self.data.len().checked_div(self.dim).unwrap_or(0)
    }

    /// Whether there are no rows.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Iterate over row slices.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &[f32]> {
        self.data.chunks_exact(self.dim)
    }
}

impl std::ops::Index<usize> for VectorRows {
    type Output = [f32];

    fn index(&self, index: usize) -> &Self::Output {
        let start = index * self.dim;
        &self.data[start..start + self.dim]
    }
}

impl<'a> IntoIterator for &'a VectorRows {
    type Item = &'a [f32];
    type IntoIter = std::slice::ChunksExact<'a, f32>;

    fn into_iter(self) -> Self::IntoIter {
        self.data.chunks_exact(self.dim)
    }
}
