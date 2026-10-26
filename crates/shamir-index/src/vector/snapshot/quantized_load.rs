//! Quantized graph loading with independent file and loader ownership.

use super::SnapshotError;
use crate::kind::VectorMetric;
use crate::vector::quantized_dist::ShamirDistU8;
use crate::vector::sq8::Sq8Quantizer;
use hnsw_rs::hnsw::Hnsw;
use hnsw_rs::hnswio::HnswIo;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

pub(super) fn load_quantized_graph(
    temp_dir: tempfile::TempDir,
    basename: &str,
    graph_bytes: &[u8],
    data_bytes: &[u8],
    quantizer: Arc<Sq8Quantizer>,
    metric: VectorMetric,
) -> Result<Arc<Hnsw<'static, u8, ShamirDistU8>>, SnapshotError> {
    let graph_path = temp_dir.path().join(format!("{basename}.hnsw.graph"));
    let data_path = temp_dir.path().join(format!("{basename}.hnsw.data"));
    let mut graph_file = std::fs::File::create(graph_path)?;
    graph_file.write_all(graph_bytes)?;
    graph_file.flush()?;
    let mut data_file = std::fs::File::create(data_path)?;
    data_file.write_all(data_bytes)?;
    data_file.flush()?;
    drop(graph_file);
    drop(data_file);

    let loader: &'static HnswIo = Box::leak(Box::new(HnswIo::new(temp_dir.path(), basename)));
    let distance = ShamirDistU8::new(quantizer, metric);
    // hnsw_rs unwraps while parsing the description, so a corrupt/truncated
    // dump panics instead of returning Err; contain it as a load error.
    let loaded = catch_unwind(AssertUnwindSafe(|| loader.load_hnsw_with_dist(distance)))
        .map_err(|_| SnapshotError::Io("hnsw_rs load_hnsw_with_dist (u8) panicked".into()))?;
    let graph = loaded.map_err(|error| {
        SnapshotError::Io(format!("hnsw_rs load_hnsw_with_dist (u8) failed: {error}"))
    })?;
    Ok(Arc::new(graph))
}
