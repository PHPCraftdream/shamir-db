//! Tests for Migration DDL: start / commit / rollback / status.

use shamir_types::mpack;

use crate::ddl;

use crate::ddl::tests::helpers::roundtrip;

// ============================================================================
// Migration DDL
// ============================================================================

#[test]
fn start_migration_wire() {
    let op = ddl::start_migration("users", "cold", "redb")
        .dst_path("/data/cold")
        .hmac("deadbeef")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "start_migration": "users",
            "repo": "main",
            "dst_repo": "cold",
            "dst_engine": "redb",
            "dst_path": "/data/cold",
            "hmac": "deadbeef"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn start_migration_minimal() {
    let op = ddl::start_migration("logs", "archive", "fjall")
        .hmac("cafe")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(j["start_migration"], "logs");
    assert_eq!(j["repo"], "main");
    assert_eq!(j["dst_repo"], "archive");
    assert_eq!(j["dst_engine"], "fjall");
    assert!(j.get("dst_path").is_none());
    assert_eq!(j["hmac"], "cafe");
}

#[test]
fn commit_migration_wire() {
    let op = ddl::commit_migration("mig-001")
        .hmac("abcd1234")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "commit_migration": "mig-001",
            "hmac": "abcd1234"
        })
    );
}

#[test]
fn rollback_migration_wire() {
    let op = ddl::rollback_migration("mig-001")
        .hmac("ff00")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "rollback_migration": "mig-001",
            "hmac": "ff00"
        })
    );
}

#[test]
fn migration_status_wire() {
    let op = ddl::migration_status("mig-001");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "migration_status": "mig-001"
        })
    );
    assert!(op.is_admin());
}

// ============================================================================
// Online backup (R2a step 3a) — node-local superuser-only admin ops
// ============================================================================

#[test]
fn start_backup_wire() {
    let op = ddl::start_backup("/backups/snap-1");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "start_backup": true,
            "to": "/backups/snap-1"
        })
    );
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn backup_status_wire() {
    let op = ddl::backup_status();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "backup_status": true
        })
    );
    assert!(op.is_admin());
    assert!(!op.is_write());
}

#[test]
fn cancel_backup_wire() {
    let op = ddl::cancel_backup();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "cancel_backup": true
        })
    );
    assert!(op.is_admin());
    assert!(op.is_write());
}
