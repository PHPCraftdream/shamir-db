//! Tests for Migration DDL: start / commit / rollback / status.

use shamir_types::mpack;

use crate::batch::IntoBatchOp;
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

// ============================================================================
// Online logical-dump export (R3 step 2b) — node-local superuser-only
// admin ops, no operator-supplied output path on the wire
// ============================================================================

#[test]
fn start_export_wire() {
    let op = ddl::start_export("appdb")
        .repo("main")
        .table("users")
        .schema_only(false)
        .include_artifacts(false)
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "start_export": true,
            "database": "appdb",
            "repo": "main",
            "table": "users",
            "schema_only": false,
            "include_artifacts": false
        })
    );
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn start_export_minimal_defaults_false() {
    let op = ddl::start_export("appdb").repo("main").build();
    let j = roundtrip(&op);
    assert_eq!(j["start_export"], true);
    assert_eq!(j["database"], "appdb");
    assert_eq!(j["repo"], "main");
    assert!(j.get("table").is_none());
    assert_eq!(j["schema_only"], false);
    assert_eq!(j["include_artifacts"], false);
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn start_export_table_without_repo_wire_shape() {
    // Table without repo still round-trips on the wire (the builder is
    // infallible, mirroring StartMigration). No scope check enforces the
    // repo/table pairing in this phase — the online job is disabled.
    let op = ddl::start_export("appdb").table("users").build();
    let j = roundtrip(&op);
    assert!(j.get("repo").is_none());
    assert_eq!(j["table"], "users");
}

#[test]
fn export_status_wire() {
    let op = ddl::export_status();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "export_status": true
        })
    );
    assert!(op.is_admin());
    assert!(!op.is_write());
}

#[test]
fn cancel_export_wire() {
    let op = ddl::cancel_export();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "cancel_export": true
        })
    );
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn start_export_into_batch_op() {
    // The infallible builder is accepted directly by `Batch::op` via
    // `IntoBatchOp` (no required field, so no `try_op` gating needed).
    let op = ddl::start_export("appdb").into_batch_op();
    assert!(op.is_admin());
    assert!(op.is_write());
}
