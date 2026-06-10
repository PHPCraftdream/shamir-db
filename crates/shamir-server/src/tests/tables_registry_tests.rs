use tempfile::TempDir;

use crate::tables_registry::TablesRegistry;

#[test]
fn open_missing_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let r = TablesRegistry::open(tmp.path()).unwrap();
    assert!(r.snapshot().tables_by_repo.is_empty());
}

#[test]
fn add_and_persist_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let r = TablesRegistry::open(tmp.path()).unwrap();
    r.add("default", "main", "widgets").unwrap();
    r.add("default", "main", "orders").unwrap();
    r.add("default", "archive", "events").unwrap();

    // Reopen — file picked up.
    let r2 = TablesRegistry::open(tmp.path()).unwrap();
    let snap = r2.snapshot();
    assert_eq!(
        snap.tables_by_repo.get("default.main").unwrap(),
        &vec!["orders".to_string(), "widgets".to_string()],
        "tables sorted"
    );
    assert_eq!(
        snap.tables_by_repo.get("default.archive").unwrap(),
        &vec!["events".to_string()]
    );

    let entries: Vec<_> = snap.iter_entries().collect();
    assert!(entries.contains(&("default", "main", "orders")));
    assert!(entries.contains(&("default", "main", "widgets")));
    assert!(entries.contains(&("default", "archive", "events")));
}

#[test]
fn add_idempotent() {
    let tmp = TempDir::new().unwrap();
    let r = TablesRegistry::open(tmp.path()).unwrap();
    r.add("d", "r", "t").unwrap();
    r.add("d", "r", "t").unwrap();
    let snap = r.snapshot();
    assert_eq!(snap.tables_by_repo.get("d.r").unwrap().len(), 1);
}

#[test]
fn remove_persists() {
    let tmp = TempDir::new().unwrap();
    let r = TablesRegistry::open(tmp.path()).unwrap();
    r.add("d", "r", "t1").unwrap();
    r.add("d", "r", "t2").unwrap();
    r.remove("d", "r", "t1").unwrap();
    let r2 = TablesRegistry::open(tmp.path()).unwrap();
    assert_eq!(
        r2.snapshot().tables_by_repo.get("d.r").unwrap(),
        &vec!["t2".to_string()]
    );
}
