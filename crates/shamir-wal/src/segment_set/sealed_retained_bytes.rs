use super::set::*;

pub(super) async fn measure(segment_set: &SegmentSet) -> DbResult<u64> {
    let gate = segment_set.lock_truncation().await;
    let paths: Vec<PathBuf> = {
        let guard = segment_set
            .inner
            .lock()
            .expect("SegmentSet inner mutex poisoned");
        guard.sealed.iter().map(|meta| meta.path.clone()).collect()
    };
    spawn_blocking(move || {
        let _gate = gate;
        paths.into_iter().try_fold(0u64, |total, path| {
            let size = std::fs::metadata(&path)
                .map_err(|error| DbError::Storage(format!("WAL metadata {path:?}: {error}")))?
                .len();
            total
                .checked_add(size)
                .ok_or_else(|| DbError::Storage("WAL retained byte count overflow".into()))
        })
    })
    .await
    .map_err(|error| DbError::Internal(format!("WAL metadata join: {error}")))?
}
