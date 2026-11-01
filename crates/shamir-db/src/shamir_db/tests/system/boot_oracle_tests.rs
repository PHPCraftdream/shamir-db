#[cfg(feature = "fjall")]
use std::sync::{Mutex, OnceLock};

#[cfg(feature = "fjall")]
use crate::access::ResourceMeta;
#[cfg(feature = "fjall")]
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
#[cfg(feature = "fjall")]
use crate::shamir_db::{ShamirDb, SystemStoreConfig};
#[cfg(feature = "fjall")]
use log::{Level, LevelFilter, Log, Metadata, Record};

#[cfg(feature = "fjall")]
static WARNINGS: OnceLock<Mutex<Vec<(Level, String)>>> = OnceLock::new();
#[cfg(feature = "fjall")]
static LOGGER: WarningLogger = WarningLogger;

#[cfg(feature = "fjall")]
struct WarningLogger;

#[cfg(feature = "fjall")]
impl Log for WarningLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            WARNINGS
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .expect("warning logger mutex")
                .push((record.level(), record.args().to_string()));
        }
    }

    fn flush(&self) {}
}

#[cfg(feature = "fjall")]
#[tokio::test]
async fn boot_warns_once_for_unknown_repository_factory_and_keeps_other_repositories() {
    let capturing = log::set_logger(&LOGGER).is_ok();
    if capturing {
        log::set_max_level(LevelFilter::Warn);
        WARNINGS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .expect("warning list")
            .clear();
    }

    let root = tempfile::tempdir().expect("temporary directory");
    let system_path = root.path().join("system");
    let database = "boot_db";
    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
            .await
            .expect("initial boot");
        db.create_db(database).await.expect("create database");
        db.add_repo(
            database,
            RepoConfig::new("available", BoxRepoFactory::in_memory()),
        )
        .await
        .expect("available repository");
        db.add_repo(
            database,
            RepoConfig::new("missing", BoxRepoFactory::in_memory()),
        )
        .await
        .expect("repository to alter");
        db.system_store()
            .save_repository(
                database,
                "missing",
                "unknown_engine",
                None,
                &ResourceMeta::open(),
                None,
            )
            .await
            .expect("rewrite persisted repository engine");
        db.system_store()
            .flush_buffers_with_quiesce_timeout(std::time::Duration::from_secs(5))
            .await
            .expect("flush system catalogue");
    }

    let reopened = reinit_with_retry(system_path).await;
    assert!(reopened
        .get_db(database)
        .expect("database")
        .get_repo("available")
        .is_some());
    assert!(reopened
        .get_db(database)
        .expect("database")
        .get_repo("missing")
        .is_none());

    if capturing {
        let warnings = WARNINGS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .expect("warning list");
        let matching: Vec<_> = warnings
            .iter()
            .filter(|(_, message)| message.contains("skipping repository 'boot_db/missing'"))
            .collect();
        assert_eq!(matching.len(), 1, "expected one warning, got {warnings:?}");
        let (level, message) = matching[0];
        assert_eq!(*level, Level::Warn, "{message}");
        assert!(message.contains("backend='unknown_engine'"), "{message}");
        assert!(message.contains("no factory available"), "{message}");
    }
}

#[cfg(feature = "fjall")]
async fn reinit_with_retry(path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(path.clone())).await {
            Ok(db) => return db,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Fjall(path))
        .await
        .expect("system store still locked after retries")
}
