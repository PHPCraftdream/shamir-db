//! Crash-image tests (acceptance test 10, PR6b-3): a legacy crash image —
//! floor marker plus v2/u64 bookmark bytes written directly into the store —
//! is REFUSED with the typed error on the cold open/apply path, and the
//! refusal REPEATS: nothing is rewritten, no v3 is minted, counters and
//! floor are unchanged. There is no runnable legacy executor to replay it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{self, Stream};
use shamir_storage::error::{DbError, DbResult, FollowerRefusalReason};
use shamir_storage::types::{RecordKey, Store};
use std::pin::Pin;

use crate::meta::recovery_marker::{load_last_committed, save_last_committed_flushed};
use crate::meta::repl::bookmark_v3::{
    classify_follower_bookmark, load_bookmark_v3, FollowerBookmarkState,
};
use crate::meta::repl_bookmark::{
    load_clean_horizon, load_replication_bookmark, save_fence_record_v3_aware, FenceRecord,
};
use crate::meta::{MetaEnvelope, MetaKey};

/// A durable crash image: an immutable byte snapshot of a metadata store.
#[derive(Default)]
struct CrashImage {
    records: Mutex<BTreeMap<RecordKey, Bytes>>,
}

impl CrashImage {
    fn set(&self, key: RecordKey, value: Bytes) {
        self.records.lock().unwrap().insert(key, value);
    }

    fn image(&self) -> Arc<dyn Store> {
        Arc::new(ImageStore {
            records: Mutex::new(self.records.lock().unwrap().clone()),
        })
    }
}

/// Read/write view of a crash image. Writes that MUTATE the image fail (the
/// refusal path must never rewrite refused state); only count them.
struct ImageStore {
    records: Mutex<BTreeMap<RecordKey, Bytes>>,
}

#[async_trait]
impl Store for ImageStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let key = RecordKey::from_slice(&(self.records.lock().unwrap().len() as u64).to_be_bytes());
        self.set(key.clone(), value).await?;
        Ok(key)
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        Ok(self.records.lock().unwrap().insert(key, value).is_none())
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.records
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| DbError::NotFound("crash image key".into()))
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        Ok(self.records.lock().unwrap().remove(&key).is_some())
    }
    // The crash image is a metadata-only snapshot: the scanned key spaces are
    // empty by construction, so an empty stream is the honest answer.
    fn iter_stream(
        &self,
        _batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        Box::pin(stream::empty())
    }
    fn scan_prefix_stream(
        &self,
        _prefix: Bytes,
        _batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        Box::pin(stream::empty())
    }
}

fn bookmark_key() -> RecordKey {
    RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes())
}

fn fence(bookmark: u64) -> FenceRecord {
    FenceRecord {
        bookmark,
        lineage_id: [5; 16],
        history_epoch: 3,
        observed_identity: None,
    }
}

/// Hand-built legacy bookmark bytes: fixed-width u64 and v2 (fence +
/// independent clean horizon), including bookmark zero.
fn legacy_bookmark_fixtures() -> Vec<(Vec<u8>, &'static str)> {
    vec![
        (
            MetaEnvelope::new(10u64).encode().unwrap(),
            "fixed-width u64",
        ),
        (MetaEnvelope::new(0u64).encode().unwrap(), "u64 zero"),
        (
            MetaEnvelope::new((2u16, (Some(fence(10)), 10u64, Some(8u64))))
                .encode()
                .unwrap(),
            "v2",
        ),
    ]
}

/// One legacy crash image: floor marker + the legacy bookmark bytes.
fn legacy_crash_image(bookmark_bytes: &[u8]) -> Arc<dyn Store> {
    let image = CrashImage::default();
    image.set(bookmark_key(), Bytes::from(bookmark_bytes.to_vec()));
    image.image()
}

async fn assert_refused_repeatedly(
    image: &Arc<dyn Store>,
    expected_bytes: &[u8],
    reason: FollowerRefusalReason,
    label: &str,
) {
    for attempt in 0..2 {
        let check = |result: DbError| {
            assert!(
                matches!(result, DbError::FollowerStateRefused { reason: r } if r == reason),
                "attempt {attempt} for {label}: expected FollowerStateRefused({reason:?}), \
                 got {result:?}"
            );
        };
        check(load_bookmark_v3(image).await.unwrap_err());
        check(classify_follower_bookmark(image).await.unwrap_err());
        check(load_replication_bookmark(image).await.unwrap_err());
        check(load_clean_horizon(image).await.unwrap_err());
        // The refusal path must NEVER mint a v3 record over refused bytes.
        check(
            save_fence_record_v3_aware(image, fence(99))
                .await
                .unwrap_err(),
        );
        // The stored bytes are untouched — no rewrite, no adoption.
        assert_eq!(
            image.get(bookmark_key()).await.unwrap().as_ref(),
            expected_bytes,
            "attempt {attempt} for {label}"
        );
    }
}

/// A legacy bookmark record in a crash image refuses every loader and the
/// v3 writer, repeatedly, with `LegacyBookmarkFormat`; nothing is rewritten.
#[tokio::test]
async fn legacy_bookmark_crash_images_refuse_repeatedly_without_rewrite() {
    for (bytes, label) in legacy_bookmark_fixtures() {
        let image = legacy_crash_image(&bytes);
        assert_refused_repeatedly(
            &image,
            &bytes,
            FollowerRefusalReason::LegacyBookmarkFormat,
            label,
        )
        .await;
    }
}

/// A floor-without-bookmark crash image is NOT a repo open: classification
/// correctly yields `FloorWithoutBookmark` (a LEADER with this state must
/// still open — covered by
/// `leader_with_floor_marker_still_opens_and_preserves_floor`). The
/// FOLLOWER-ATTEMPT adoption over the image (`save_fence_record_v3_aware`)
/// refuses with `LegacyFollowerState`, repeatedly, with the floor preserved
/// verbatim and no v3 record minted.
#[tokio::test]
async fn floor_only_crash_image_classifies_unowned_and_refuses_adoption() {
    let image = CrashImage::default();
    let store = image.image();
    // Floor marker WITHOUT any bookmark: unowned pre-receipt follower state.
    save_last_committed_flushed(&store, 7).await.unwrap();
    for attempt in 0..2 {
        assert!(
            matches!(
                classify_follower_bookmark(&store).await.unwrap(),
                FollowerBookmarkState::FloorWithoutBookmark { floor: 7 }
            ),
            "attempt {attempt}"
        );
        let error = save_fence_record_v3_aware(&store, fence(99))
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                DbError::FollowerStateRefused {
                    reason: FollowerRefusalReason::LegacyFollowerState
                }
            ),
            "attempt {attempt}: {error:?}"
        );
        assert_eq!(load_last_committed(&store).await.unwrap(), Some(7));
        assert!(load_bookmark_v3(&store).await.unwrap().is_none());
    }
}

/// A crash image containing BOTH a valid v3 record and its floor keeps
/// opening as a native follower — the only supported durable state.
#[tokio::test]
async fn v3_crash_image_still_classifies_native() {
    let image = CrashImage::default();
    let store = image.image();
    save_last_committed_flushed(&store, 2).await.unwrap();
    crate::meta::repl::bookmark_v3::save_bookmark_v3(
        &store,
        &crate::meta::repl::bookmark_v3::BookmarkV3 {
            fence: Some(fence(2)),
            bookmark: 2,
            clean_through: Some(2),
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        classify_follower_bookmark(&store).await.unwrap(),
        crate::meta::repl::bookmark_v3::FollowerBookmarkState::Native(_)
    ));
    assert_eq!(load_replication_bookmark(&store).await.unwrap(), 2);
    assert_eq!(load_clean_horizon(&store).await.unwrap(), Some(2));
}
