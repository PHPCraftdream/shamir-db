use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bench_scale_tool::Harness;
use shamir_db::{ShamirDb, SystemStoreConfig};

pub(super) fn function_wasm(i: usize) -> Vec<u8> {
    let value = i32::try_from(i).expect("function count fits wasm i32");
    let wat = format!(
        r#"(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.shl (i64.const {}) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))
  )
  (func (export "f") (param i32) (result i32)
    (i32.add (local.get 0) (i32.const {})))
)"#,
        value, value
    );
    wat::parse_str(&wat).expect("valid function wat")
}

fn has_entry_files(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_file() {
            path.extension().is_some_and(|ext| ext == "e")
        } else if path.is_dir() && entry.file_name() != "tmp" {
            has_entry_files(&path)
        } else {
            false
        }
    })
}

fn assert_named_functions(db: &ShamirDb, k: usize, phase: &str) {
    let names = db.functions().list();
    for i in 0..k {
        let expected = format!("f_boot_{i}");
        assert!(
            names.iter().any(|name| name == &expected),
            "{phase}: {expected} missing after open"
        );
    }
}

async fn seed_db(path: &Path, k: usize) -> ShamirDb {
    let db = ShamirDb::init(SystemStoreConfig::Fjall(path.to_path_buf()))
        .await
        .expect("seed db init");
    for i in 0..k {
        db.create_function_from_wasm(&format!("f_boot_{i}"), &function_wasm(i), false)
            .await
            .expect("create function");
    }
    assert_named_functions(&db, k, "seed");
    db
}

pub(super) fn register_cells(h: &mut Harness) {
    for k in [16usize, 64] {
        let cold_id = format!("boot_k_functions/cold/{k}");
        let cold_previous = Rc::new(RefCell::new(None));
        let cold_cleanup = cold_previous.clone();
        h.bench_batched_async(
            &cold_id,
            move || {
                let previous = cold_cleanup.clone();
                async move {
                    if let Some((db, dir)) = previous.borrow_mut().take() {
                        drop(db);
                        drop(dir);
                    }
                    let tempdir = tempfile::TempDir::new().expect("tempdir");
                    let root = tempdir.path().join("seed");
                    std::fs::create_dir_all(&root).expect("seed root");
                    let system_path = root.join("system");
                    let seeded = seed_db(&system_path, k).await;
                    drop(seeded);
                    let cache = root.join("cache");
                    if cache.exists() {
                        std::fs::remove_dir_all(cache).expect("clear seed cache");
                    }
                    (system_path, tempdir)
                }
            },
            {
                let previous = cold_previous.clone();
                move |(path, tempdir)| {
                    let previous = previous.clone();
                    async move {
                        let db = ShamirDb::init(SystemStoreConfig::Fjall(path))
                            .await
                            .expect("cold reopen");
                        assert_named_functions(&db, k, "cold reopen");
                        std::hint::black_box(&db);
                        *previous.borrow_mut() = Some((db, tempdir));
                    }
                }
            },
        );

        let warm_id = format!("boot_k_functions/warm/{k}");
        let warm_previous = Rc::new(RefCell::new(None));
        let warm_cleanup = warm_previous.clone();
        h.bench_batched_async(
            &warm_id,
            move || {
                let previous = warm_cleanup.clone();
                async move {
                    if let Some((db, dir)) = previous.borrow_mut().take() {
                        drop(db);
                        drop(dir);
                    }
                    let tempdir = tempfile::TempDir::new().expect("tempdir");
                    let system_path: PathBuf = tempdir.path().join("data").join("system");
                    std::fs::create_dir_all(system_path.parent().expect("data root"))
                        .expect("data root");
                    let seeded = seed_db(&system_path, k).await;
                    drop(seeded);
                    let cache = system_path.parent().expect("data root").join("cache");
                    if cache.exists() {
                        std::fs::remove_dir_all(&cache).expect("clear registration cache");
                    }
                    let first = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
                        .await
                        .expect("first warm-cache reopen");
                    assert_named_functions(&first, k, "warm cache seed reopen");
                    drop(first);
                    assert!(
                        has_entry_files(&cache.join("wasm")),
                        "warm boot seed produced no .e files under {}",
                        cache.display()
                    );
                    (system_path, tempdir)
                }
            },
            {
                let previous = warm_previous.clone();
                move |(path, tempdir)| {
                    let previous = previous.clone();
                    async move {
                        let db = ShamirDb::init(SystemStoreConfig::Fjall(path))
                            .await
                            .expect("warm reopen");
                        assert_named_functions(&db, k, "warm reopen");
                        std::hint::black_box(&db);
                        *previous.borrow_mut() = Some((db, tempdir));
                    }
                }
            },
        );
    }
}
