//! Tests for Migration DDL: start / commit / rollback / status.

use shamir_types::mpack;

use crate::ddl;

use super::helpers::roundtrip;

// ============================================================================
// Migration DDL
// ============================================================================

#[test]
fn start_migration_wire() {
    let op = ddl::start_migration("users", "cold", "redb")
        .dst_path("/data/cold")
        .hmac("deadbeef")
        .build();
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
    let op = ddl::start_migration("logs", "archive", "fjall").build();
    let j = roundtrip(&op);
    assert_eq!(j["start_migration"], "logs");
    assert_eq!(j["repo"], "main");
    assert_eq!(j["dst_repo"], "archive");
    assert_eq!(j["dst_engine"], "fjall");
    assert!(j.get("dst_path").is_none());
    assert!(j.get("hmac").is_none());
}

#[test]
fn commit_migration_wire() {
    let op = ddl::commit_migration("mig-001").hmac("abcd1234").build();
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
    let op = ddl::rollback_migration("mig-001").hmac("ff00").build();
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
