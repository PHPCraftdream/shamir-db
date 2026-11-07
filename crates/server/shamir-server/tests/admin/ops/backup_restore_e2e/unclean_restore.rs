use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;
use tokio::runtime::Builder;

use shamir_db::engine::tx::ReplicatedEvent;
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};
use shamir_server::backup;
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::restore;
use shamir_tx::ChangeOp;

use super::common::{create_table_req, launch, login, read_req, roundtrip, write_req};

#[test]
fn backup_unclean_server_restore_certifies_before_serving_pull() {
    const CHILD_DIR: &str = "SHAMIR_RECOVERY_GAP_BACKUP_CHILD_DIR";
    const PASSWORD: &[u8] = b"recovery gap backup contract password";
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    if let Some(path) = std::env::var_os(CHILD_DIR) {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let handle = launch(PathBuf::from(path), 0, PASSWORD).await;
            let (mut r, mut w, sid) =
                login(handle.first_tls_exporter_addr().unwrap(), PASSWORD).await;
            for (rid, request) in [
                create_table_req("unclean_backup_items"),
                write_req("unclean_backup_items", "CRASH", 71),
            ]
            .into_iter()
            .enumerate()
            {
                let response = roundtrip(
                    &request,
                    sid,
                    u32::try_from(rid + 1).unwrap(),
                    &mut w,
                    &mut r,
                )
                .await;
                let DbResponse::Batch { response } = response else {
                    panic!("fixture write failed: {response:?}");
                };
                assert!(!response.results.contains_key("__error"), "{response:?}");
            }
            // No ServerHandle::shutdown, flush_all or shutdown_clean. This
            // models unclean termination, not a guaranteed power-loss boundary.
            drop(w);
            drop(r);
            drop(handle);
        });
        drop(runtime);
        return;
    }

    let data = TempDir::new().unwrap();
    let destination = TempDir::new().unwrap();
    // Libtest omits the crate segment from the full test name.
    let module = module_path!().split_once("::").map_or("", |(_, m)| m);
    let exact = format!("{module}::backup_unclean_server_restore_certifies_before_serving_pull");
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &exact, "--nocapture"])
        .env(CHILD_DIR, data.path())
        .status()
        .unwrap();
    assert!(status.success(), "unclean fixture child failed: {status:?}");
    let snapshot = backup::backup(data.path(), destination.path()).unwrap();
    backup::verify_manifest(&snapshot.dest_dir).unwrap();
    let restored = restore::restore(&snapshot.dest_dir, data.path(), false).unwrap();
    assert!(restored.files_restored > 0);
    assert!(restored.pre_restore_backup.unwrap().exists());

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let handle = launch(data.path().to_path_buf(), 0, PASSWORD).await;
        let (mut r, mut w, sid) = login(handle.first_tls_exporter_addr().unwrap(), PASSWORD).await;
        let response = roundtrip(&read_req("unclean_backup_items"), sid, 1, &mut w, &mut r).await;
        let DbResponse::Batch { response } = response else {
            panic!("restored repository did not open: {response:?}");
        };
        let rows = &response.results["rd"].records;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get_value_str("sku"), Some("CRASH"));
        assert_eq!(rows[0].get_value_i64("qty"), Some(71));

        let response = roundtrip(
            &DbRequest::Repl(ReplRequest::Pull {
                db: "default".into(),
                repo: "main".into(),
                from_version: 1,
                limit: 100,
                wait_ms: None,
                capabilities: None,
            }),
            sid,
            2,
            &mut w,
            &mut r,
        )
        .await;
        let DbResponse::Repl(ReplResponse::Pull {
            gap_at,
            current_version,
            events,
            ..
        }) = response
        else {
            panic!("expected restored replication Pull: {response:?}");
        };
        assert!(current_version >= 1, "restored committed horizon");
        assert_eq!(gap_at, None, "surviving WAL commits are recoverable");
        let decoded: Vec<ReplicatedEvent> = rmp_serde::from_slice(&events).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].event.commit_version, current_version);
        assert_eq!(decoded[0].event.changes.len(), 1);
        let change = &decoded[0].event.changes[0];
        assert_eq!(change.table, "unclean_backup_items");
        assert_eq!(change.op, ChangeOp::Put);
        assert!(change.value.is_some());
        drop(w);
        drop(r);
        handle.shutdown().await;
    });
    drop(runtime);
}
