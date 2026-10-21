use crate::{CompileCache, Outcome};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use wasmtime::{Engine, Instance, Module, Store};

const ANSWER_WAT: &str = r#"
(module
  (func (export "answer") (result i32)
    i32.const 42))
"#;

const OTHER_WAT: &str = r#"
(module
  (func (export "other") (result i32)
    i32.const 7))
"#;

fn wasm(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).unwrap()
}

fn open(dir: &Path) -> CompileCache {
    CompileCache::open(dir.to_path_buf(), u64::MAX).unwrap()
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|item| item.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "cwasm"))
        .collect();
    found.sort();
    found
}

fn export_names(module: &Module) -> Vec<String> {
    module
        .exports()
        .map(|export| export.name().to_owned())
        .collect()
}

fn call_i32(engine: &Engine, module: &Module, export: &str) -> i32 {
    let mut store = Store::new(engine, ());
    let instance = Instance::new(&mut store, module, &[]).unwrap();
    let func = instance
        .get_typed_func::<(), i32>(&mut store, export)
        .unwrap();
    func.call(&mut store, ()).unwrap()
}

fn age(path: &Path, secs: u64) {
    let file = OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn second_lookup_is_served_from_the_cache_and_runs() {
    // Counterfactual: a cache that stores nothing (or an entry that does not
    // deserialize back into a runnable module) never reports a Hit.
    let tmp = tempfile::tempdir().unwrap();
    let cache = open(tmp.path());
    let engine = Engine::default();
    let bytes = wasm(ANSWER_WAT);

    let (first, outcome) = cache.module(&engine, &bytes).unwrap();
    assert_eq!(outcome, Outcome::Miss);
    assert_eq!(entries(tmp.path()).len(), 1);

    let (second, outcome) = cache.module(&engine, &bytes).unwrap();
    assert_eq!(outcome, Outcome::Hit);
    assert_eq!(export_names(&first), export_names(&second));
    assert_eq!(call_i32(&engine, &second, "answer"), 42);
    assert_eq!(entries(tmp.path()).len(), 1);
}

#[test]
fn entries_are_keyed_by_the_wasm_bytes() {
    // Counterfactual: a key that ignores the bytes would serve the first
    // module for the second one.
    let tmp = tempfile::tempdir().unwrap();
    let cache = open(tmp.path());
    let engine = Engine::default();

    let (answer, _) = cache.module(&engine, &wasm(ANSWER_WAT)).unwrap();
    let (other, outcome) = cache.module(&engine, &wasm(OTHER_WAT)).unwrap();
    assert_eq!(outcome, Outcome::Miss);
    assert_eq!(export_names(&answer), ["answer"]);
    assert_eq!(export_names(&other), ["other"]);
    assert_eq!(entries(tmp.path()).len(), 2);
}

#[test]
fn flipped_payload_byte_is_dropped_and_recompiled() {
    // Counterfactual: without the digest check a bit-rotted entry would reach
    // `Module::deserialize`.
    let tmp = tempfile::tempdir().unwrap();
    let cache = open(tmp.path());
    let engine = Engine::default();
    let bytes = wasm(ANSWER_WAT);
    cache.module(&engine, &bytes).unwrap();

    let path = entries(tmp.path()).remove(0);
    let mut entry = fs::read(&path).unwrap();
    let last = entry.len() - 1;
    entry[last] ^= 0xff;
    fs::write(&path, &entry).unwrap();

    let (module, outcome) = cache.module(&engine, &bytes).unwrap();
    assert_eq!(outcome, Outcome::Miss);
    assert_eq!(call_i32(&engine, &module, "answer"), 42);
    let (_, outcome) = cache.module(&engine, &bytes).unwrap();
    assert_eq!(outcome, Outcome::Hit, "the rewritten entry is intact");
}

#[test]
fn truncated_and_foreign_entries_are_recompiled() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = open(tmp.path());
    let engine = Engine::default();
    let bytes = wasm(ANSWER_WAT);
    cache.module(&engine, &bytes).unwrap();
    let path = entries(tmp.path()).remove(0);

    for junk in [&b""[..], &b"short"[..], &[0u8; 64][..]] {
        fs::write(&path, junk).unwrap();
        let (_, outcome) = cache.module(&engine, &bytes).unwrap();
        assert_eq!(outcome, Outcome::Miss, "junk of {} bytes", junk.len());
        let (_, outcome) = cache.module(&engine, &bytes).unwrap();
        assert_eq!(outcome, Outcome::Hit);
    }
}

#[test]
fn open_trims_the_oldest_entries_over_the_soft_limit() {
    // Counterfactual: without trimming the cache grows without bound.
    let tmp = tempfile::tempdir().unwrap();
    for (name, age_secs) in [("a", 300), ("b", 200), ("c", 100)] {
        let path = tmp.path().join(format!("{name}.cwasm"));
        fs::write(&path, [0u8; 100]).unwrap();
        age(&path, age_secs);
    }

    CompileCache::open(tmp.path().to_path_buf(), 250).unwrap();

    let names: Vec<String> = entries(tmp.path())
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["b.cwasm", "c.cwasm"], "only the oldest entry goes");
}

#[test]
fn open_removes_stale_temp_files_only() {
    let tmp = tempfile::tempdir().unwrap();
    let stale = tmp.path().join("1-1.tmp");
    let fresh = tmp.path().join("2-1.tmp");
    fs::write(&stale, b"x").unwrap();
    fs::write(&fresh, b"x").unwrap();
    age(&stale, 2 * 3600);

    open(tmp.path());

    assert!(!stale.exists(), "a crashed writer's leftover is removed");
    assert!(fresh.exists(), "a possibly live writer's file stays");
}

#[cfg(unix)]
#[test]
fn group_or_world_writable_directory_is_rejected() {
    // Counterfactual: entries run as native code; a directory other users can
    // write to would let them plant one.
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o777)).unwrap();

    let err = CompileCache::open(tmp.path().to_path_buf(), u64::MAX)
        .err()
        .unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}
