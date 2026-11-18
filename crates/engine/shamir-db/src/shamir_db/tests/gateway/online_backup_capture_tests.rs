//! Facade tests for `ShamirDb::capture_system_store_point` (R2a step 3a,
//! design `online-backup.md` Q4 / protocol step B).
//!
//! Proven: the SYSTEM repo's capture barrier runs (flush under the
//! writer-drain quiesce) and the returned version is the highest
//! fully-captured SYSTEM-repo WAL position — monotonic across catalogue
//! writes — and the flushed catalogue state survives a full close/reopen
//! of the same persistent system-store directory.

use crate::query::admin::GroupRef;
use crate::shamir_db::{ShamirDb, SystemStoreConfig};

/// Re-open the persistent system store after a dropped instance, retrying
/// while background repository handles (the drainer's clone) still hold the
/// fjall file lock. Same discipline as the catalogue fixture helpers.
async fn reopen(path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(path.clone())).await {
            Ok(db) => return db,
            Err(e) if e.to_string().contains("Locked") => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(e) => panic!("reopen failed: {e}"),
        }
    }
    panic!("fjall lock was not released within the retry window");
}

/// The facade returns a monotonic catalogue write horizon: two catalogue
/// writes produce v1 < v2, and the flushed state is durable — after
/// closing every handle and reopening the same fjall directory, the
/// written group rows are present.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capture_system_store_point_is_monotonic_and_durable() {
    let root = tempfile::tempdir().expect("tempdir").keep();
    let store_path = root.join("system");

    let db = ShamirDb::init(SystemStoreConfig::Fjall(store_path.clone()))
        .await
        .expect("init engine");
    db.create_group("alpha").await.expect("create group alpha");
    let v1 = db
        .capture_system_store_point()
        .await
        .expect("first capture point");
    let alpha_id = db
        .resolve_group_id(&GroupRef::Name {
            name: "alpha".to_string(),
        })
        .await
        .expect("resolve alpha");

    db.create_group("beta").await.expect("create group beta");
    let v2 = db
        .capture_system_store_point()
        .await
        .expect("second capture point");
    assert!(
        v2 > v1,
        "the catalogue write horizon must advance across catalogue writes: {v1} !< {v2}"
    );

    // Close/drop every handle, then reopen the same directory.
    drop(db);
    let reopened = reopen(store_path.clone()).await;
    let resolved = reopened
        .resolve_group_id(&GroupRef::Name {
            name: "alpha".to_string(),
        })
        .await
        .expect("resolve alpha after reopen");
    assert_eq!(
        resolved, alpha_id,
        "the flushed catalogue row must survive the restart"
    );
    assert!(
        reopened
            .resolve_group_id(&GroupRef::Name {
                name: "beta".to_string(),
            })
            .await
            .is_ok(),
        "the second flushed catalogue row must survive the restart"
    );
    drop(reopened);
    let _ = std::fs::remove_dir_all(&root);
}
