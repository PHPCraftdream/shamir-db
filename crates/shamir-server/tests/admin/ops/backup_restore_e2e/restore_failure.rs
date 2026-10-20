//! CR-B7: pre-swap ticket invalidation reordering — a failure at the
//! invalidation step (now staged BEFORE the atomic swap) must leave the
//! CURRENT (pre-restore) data_dir completely untouched, with no
//! `.pre_restore_backup_*` sibling created at all (the swap never runs).

use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use shamir_server::backup;
use shamir_server::db_handler::DbResponse;
use shamir_server::restore;

use crate::common::{create_table_req, launch, login, read_req, roundtrip, write_req};

/// Snapshot every file's relative path + contents under `dir`, so a later
/// call can assert nothing changed at all (not just "same file count").
/// `Err` when a file vanishes mid-walk (a store still rewriting its files).
fn try_snapshot_dir_contents(
    dir: &std::path::Path,
) -> std::io::Result<std::collections::BTreeMap<String, Vec<u8>>> {
    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut std::collections::BTreeMap<String, Vec<u8>>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&path)?);
            }
        }
        Ok(())
    }
    let mut out = std::collections::BTreeMap::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

fn snapshot_dir_contents(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    try_snapshot_dir_contents(dir).unwrap()
}

/// Snapshot `dir` once it has stopped changing. `ServerHandle::shutdown()`
/// can return while the stopped server's fjall handles are still closing
/// (the final flush and a compaction pass rewrite SST files for a moment), so
/// a snapshot taken right after it can differ from the same directory a
/// moment later although nothing touched it. Wait for 1s of no change.
async fn quiescent_snapshot(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut last = None;
    let mut stable = 0u32;
    for _ in 0..150 {
        let now = try_snapshot_dir_contents(dir).ok();
        match (&now, &last) {
            (Some(cur), Some(prev)) if cur == prev => {
                stable += 1;
                if stable >= 5 {
                    return cur.clone();
                }
            }
            _ => stable = 0,
        }
        last = now;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("data_dir kept changing for 30s after the server shut down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_failure_at_pre_swap_invalidation_leaves_data_dir_untouched() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let password: &[u8] = b"correct horse battery staple CR-B7";

    let data_temp = TempDir::new().unwrap();
    let data_dir = data_temp.path().to_path_buf();

    // ---- Boot #1: create table, write a row, then shut down cleanly. ----
    {
        let handle = launch(data_dir.clone(), 0, password).await;
        let addr = handle.first_tls_exporter_addr().unwrap();
        let (mut r, mut w, sid) = login(addr, password).await;

        let res = roundtrip(&create_table_req("crb7_items"), sid, 1, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "create_table");
        let res = roundtrip(&write_req("crb7_items", "ORIG", 7), sid, 2, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "write");

        let _ = w.shutdown().await;
        let mut tmp = [0u8; 1];
        let _ = r.read(&mut tmp).await;
        handle.shutdown().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Snapshot the CURRENT (pre-restore) data_dir contents in full, so we
    // can later assert byte-for-byte that nothing changed.
    let pre_restore_snapshot = quiescent_snapshot(&data_dir).await;
    assert!(
        !pre_restore_snapshot.is_empty(),
        "sanity: data_dir must contain files before the failed restore attempt"
    );

    // ---- Backup: snapshot the current state ----
    let backup_dest = TempDir::new().unwrap();
    let report = backup::backup(&data_dir, backup_dest.path()).expect("backup ok");

    // ---- Corrupt the SNAPSHOT's `users` store so the pre-swap
    // invalidation step (which opens `temp_dir/users`, a COPY of this
    // snapshot's `users`) fails structurally, before the swap ever runs.
    // The corrupted bytes' manifest entries are patched IN PLACE (same
    // relative path, freshly-computed sha256/size for the corrupted
    // content) so `verify_manifest` (Step 2, which runs BEFORE the copy)
    // still passes — the failure this test targets must occur at the NEW
    // pre-swap invalidation step (Step 4), not at manifest verification. ----
    let snapshot_users_dir = report.dest_dir.join("users");
    assert!(
        snapshot_users_dir.is_dir(),
        "sanity: snapshot must contain a users/ directory to corrupt"
    );
    let corrupted_bytes: &[u8] = b"CR-B7 deliberately corrupted, not a valid fjall file";
    let mut corrupted_rel_paths: Vec<String> = Vec::new();
    fn corrupt_recursive(
        root: &std::path::Path,
        dir: &std::path::Path,
        corrupted_bytes: &[u8],
        corrupted_rel_paths: &mut Vec<String>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                corrupt_recursive(root, &path, corrupted_bytes, corrupted_rel_paths);
            } else if path.is_file() {
                std::fs::write(&path, corrupted_bytes).unwrap();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                corrupted_rel_paths.push(rel);
            }
        }
    }
    corrupt_recursive(
        &report.dest_dir,
        &snapshot_users_dir,
        corrupted_bytes,
        &mut corrupted_rel_paths,
    );
    assert!(
        !corrupted_rel_paths.is_empty(),
        "sanity: at least one users/ file must have been corrupted"
    );

    let manifest_path = report.dest_dir.join(backup::MANIFEST_FILE_NAME);
    let manifest_raw = std::fs::read(&manifest_path).unwrap();
    let mut manifest: backup::Manifest = serde_json::from_slice(&manifest_raw).unwrap();
    let corrupted_sha256 = hex::encode(shamir_connect::common::crypto::sha256(corrupted_bytes));
    let corrupted_size = corrupted_bytes.len() as u64;
    for entry in &mut manifest.files {
        if corrupted_rel_paths.contains(&entry.path) {
            entry.sha256 = corrupted_sha256.clone();
            entry.size_bytes = corrupted_size;
        }
    }
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // Sanity: the patched manifest verifies cleanly against the corrupted
    // (but now self-consistent) snapshot — otherwise this test would be
    // exercising manifest verification (Step 2), not the pre-swap
    // invalidation step (Step 4) it's meant to target.
    backup::verify_manifest(&report.dest_dir)
        .expect("patched manifest must verify against the corrupted snapshot content");

    // ---- Restore MUST fail at the pre-swap invalidation step (the
    // corrupted `users` store cannot be opened as a fjall keyspace). Uses
    // `force: true` to skip the Step 1 liveness probe, which otherwise
    // legitimately mutates the LIVE data_dir's own server_meta
    // (`touch_last_started_at` + fjall's own journal-truncation-on-open) —
    // that mutation is pre-existing Step-1 behaviour, unrelated to this
    // task's reordering fix, and would otherwise pollute the
    // byte-for-byte comparison below with an unrelated diff. ----
    let err = restore::restore(&report.dest_dir, &data_dir, true).unwrap_err();
    eprintln!("restore failed as expected: {err}");

    // ---- Core guarantee: data_dir is BYTE-FOR-BYTE unchanged, and no
    // `.pre_restore_backup_*` sibling was created (the swap never ran). ----
    let post_failure_snapshot = snapshot_dir_contents(&data_dir);
    if pre_restore_snapshot != post_failure_snapshot {
        let mut diffs = Vec::new();
        let all_keys: std::collections::BTreeSet<&String> = pre_restore_snapshot
            .keys()
            .chain(post_failure_snapshot.keys())
            .collect();
        for key in all_keys {
            let before = pre_restore_snapshot.get(key);
            let after = post_failure_snapshot.get(key);
            if before != after {
                diffs.push(format!(
                    "{key}: before={:?} bytes, after={:?} bytes",
                    before.map(|v| v.len()),
                    after.map(|v| v.len())
                ));
            }
        }
        panic!(
            "data_dir must be completely untouched after a failed restore \
             (pre-swap invalidation failure must abort before the atomic swap); \
             differing entries: {diffs:#?}"
        );
    }

    // Scoped to a sibling name derived from THIS test's own data_dir name
    // (`restore.rs`'s naming scheme is `{dir_name}.pre_restore_backup_{stamp}`)
    // — `parent` is the shared OS temp root, so a plain
    // `.pre_restore_backup_` substring scan would also match siblings
    // legitimately created by OTHER tests running concurrently in this
    // same file (e.g. `restore_cli_rolls_back_data_and_invalidates_tickets`).
    let parent = data_dir.parent().unwrap().to_path_buf();
    let data_dir_name = data_dir.file_name().unwrap().to_string_lossy().into_owned();
    let sibling_prefix = format!("{data_dir_name}.pre_restore_backup_");
    let found_pre_restore_sibling = std::fs::read_dir(&parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().starts_with(&sibling_prefix));
    assert!(
        !found_pre_restore_sibling,
        "no .pre_restore_backup_* sibling must exist for this test's data_dir — the swap never ran"
    );

    // ---- Regression: the server can still boot normally against the
    // untouched, original data_dir. ----
    let handle = launch(data_dir.clone(), 0, password).await;
    let addr = handle.first_tls_exporter_addr().unwrap();
    let (mut r, mut w, sid) = login(addr, password).await;
    let res = roundtrip(&read_req("crb7_items"), sid, 1, &mut w, &mut r).await;
    match res {
        DbResponse::Batch { response } => {
            let rd = response.results.get("rd").expect("rd alias must exist");
            assert_eq!(rd.records.len(), 1, "original data must still be intact");
            assert_eq!(rd.records[0].get_value_str("sku"), Some("ORIG"));
        }
        other => panic!(
            "expected Batch after untouched data_dir reboot, got {:?}",
            other
        ),
    }
    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
    handle.shutdown().await;
}
