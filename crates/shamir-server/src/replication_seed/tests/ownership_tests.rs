use shamir_db::{ShamirDb, SystemStoreConfig};
use tempfile::TempDir;

use crate::replication_seed::maintenance_open::MaintenanceOpen;
use crate::replication_seed::SeedError;

#[tokio::test]
async fn server_ownership_blocks_export_before_catalogue_open() {
    let dir = TempDir::new().unwrap();
    let server = crate::maintenance_lock::acquire(dir.path()).unwrap();
    let out = dir.path().join("seed");
    let error = crate::replication_seed::export(dir.path(), "app", "main", &out)
        .await
        .unwrap_err();
    #[cfg(unix)]
    assert!(matches!(error, crate::replication_seed::SeedError::Io(_)));
    #[cfg(not(unix))]
    assert!(error.to_string().contains("Unix"));
    assert!(!out.exists());
    drop(server);
}

#[test]
fn valid_catalogue_maintenance_owner_blocks_second_open() {
    let dir = TempDir::new().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let meta = dir.path().join("shamir_db_meta.redb");
        let source = ShamirDb::init(SystemStoreConfig::Fjall(meta))
            .await
            .unwrap();
        source.flush_all().await.unwrap();
    });
    drop(runtime);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let held = crate::maintenance_lock::acquire(dir.path()).unwrap();
        assert!(matches!(
            MaintenanceOpen::open(dir.path()).await,
            Err(SeedError::Io(_)),
        ));
        drop(held);
        let owner = MaintenanceOpen::open(dir.path()).await.unwrap();
        assert!(matches!(
            MaintenanceOpen::open(dir.path()).await,
            Err(SeedError::Io(_)),
        ));
        assert!(crate::maintenance_lock::acquire_server(dir.path()).is_err());
        drop(owner);
    });
    drop(runtime);
    assert!(crate::maintenance_lock::acquire(dir.path()).is_ok());
}

#[test]
fn shared_lock_refreshes_pid_and_preserves_server_io_errors() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(".shamir.lock");
    std::fs::write(&path, b"stale-long-pid-diagnostics").unwrap();
    let owner = crate::maintenance_lock::acquire(dir.path()).unwrap();
    let pid = std::process::id().to_string();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), pid);
    assert!(matches!(
        crate::maintenance_lock::acquire_server(dir.path()),
        Err(crate::server::BootError::AlreadyRunning(_)),
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), pid);
    drop(owner);
    assert!(matches!(
        crate::maintenance_lock::acquire_server(&dir.path().join("missing")),
        Err(crate::server::BootError::Io(_)),
    ));
}

#[test]
fn export_ownership_blocks_the_server_shared_acquisition() {
    let dir = TempDir::new().unwrap();
    let exporter = crate::maintenance_lock::acquire(dir.path()).unwrap();
    assert!(crate::maintenance_lock::acquire(dir.path()).is_err());
    drop(exporter);
    let server = crate::maintenance_lock::acquire(dir.path()).unwrap();
    drop(server);
}
