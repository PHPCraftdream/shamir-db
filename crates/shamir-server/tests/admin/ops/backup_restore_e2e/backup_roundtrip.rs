//! Backup → wipe → restore → reboot: the data written before the backup
//! must survive and the server must reboot on the restored data_dir.

use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use shamir_server::backup;
use shamir_server::db_handler::DbResponse;

use super::common::{create_table_req, launch, login, read_req, roundtrip, write_req};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backup_then_restore_recovers_data() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let password: &[u8] = b"correct horse battery staple";

    // ---- Boot #1: create + write ----
    let data_temp = TempDir::new().unwrap();
    let data_dir = data_temp.path().to_path_buf();

    let port: u16 = {
        let handle = launch(data_dir.clone(), 0, password).await;
        let addr = handle.first_tls_exporter_addr().unwrap();
        let port = addr.port();

        let (mut r, mut w, sid) = login(addr, password).await;

        let res = roundtrip(&create_table_req("backup_items"), sid, 1, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "create_table");

        let res = roundtrip(&write_req("backup_items", "X1", 42), sid, 2, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "write");

        let _ = w.shutdown().await;
        let mut tmp = [0u8; 1];
        let _ = r.read(&mut tmp).await;
        handle.shutdown().await;
        // Give the OS a moment to settle pending fsyncs / file-handle drops.
        // The Drop chain (Arc<ShamirDb> → DbInstance → RepoInstance →
        // redb Database) closes the redb files; copying them only after
        // every Drop has run is the only way to guarantee the snapshot
        // sees the post-write state on Windows where mmap regions linger.
        tokio::time::sleep(Duration::from_millis(200)).await;
        port
    };

    // Diagnostic: list what's in data_dir before backup so a regression
    // shows which file is missing.
    let pre_backup_files: Vec<String> = std::fs::read_dir(&data_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    eprintln!("data_dir before backup: {:?}", pre_backup_files);
    let wt = std::fs::read_to_string(data_dir.join("wire_tables.mpack")).unwrap_or_default();
    eprintln!("wire_tables.mpack: {}", wt);
    let main_size = std::fs::metadata(data_dir.join("shamir_db_default_main.redb"))
        .map(|m| m.len())
        .unwrap_or(0);
    eprintln!("default_main.redb size: {} bytes", main_size);

    // ---- Backup ----
    let backup_dest = TempDir::new().unwrap();
    let report = backup::backup(&data_dir, backup_dest.path()).expect("backup ok");
    assert!(
        report.files_copied > 0,
        "backup must have copied at least one file"
    );

    // ---- "Disaster" — wipe data_dir contents (but keep the directory
    //      itself, so we restore back to the SAME path). ----
    //
    // Restoring to the SAME absolute path is required because ShamirDb's
    // system_store records each repo's data file as an absolute path. A
    // future patch could rewrite paths on boot to make migration to a
    // different `data_dir` work; for now the disaster-recovery contract
    // is "stop server, restore files to the same place, restart".
    for entry in std::fs::read_dir(&data_dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            std::fs::remove_dir_all(&p).unwrap();
        } else {
            std::fs::remove_file(&p).unwrap();
        }
    }
    // Sanity: data_dir is now empty.
    assert_eq!(std::fs::read_dir(&data_dir).unwrap().count(), 0);

    // ---- Restore: copy snapshot back into the same data_dir ----
    fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let target = dst.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir_recursive(&path, &target)?;
            } else {
                std::fs::copy(&path, &target)?;
            }
        }
        Ok(())
    }
    copy_dir_recursive(&report.dest_dir, &data_dir).expect("restore copy");

    // ---- Boot #2 against the restored (same) data_dir ----
    let handle = launch(data_dir.clone(), port, password).await;
    let addr = handle.first_tls_exporter_addr().unwrap();
    assert_eq!(addr.port(), port, "second boot reuses same port");

    let (mut r, mut w, sid) = login(addr, password).await;
    let res = roundtrip(&read_req("backup_items"), sid, 1, &mut w, &mut r).await;
    match res {
        DbResponse::Batch { response } => {
            let rd = response
                .results
                .get("rd")
                .expect("rd alias must exist after restore");
            assert_eq!(
                rd.records.len(),
                1,
                "exactly one record must survive backup → wipe → restore"
            );
            assert_eq!(rd.records[0].get_value_str("sku"), Some("X1"));
            assert_eq!(rd.records[0].get_value_i64("qty"), Some(42));
        }
        DbResponse::Error { code, message } => {
            panic!("expected restored data, got error {{ code: {code:?}, message: {message:?} }}");
        }
        other => panic!("expected Batch, got {:?}", other),
    }

    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
    handle.shutdown().await;
}
