use std::fs;

#[cfg(unix)]
use shamir_db::engine::index2::base_index::sorted_index_definition::SortedIndexDefinition;
#[cfg(unix)]
use shamir_db::engine::index2::descriptor::IndexDescriptor;
#[cfg(unix)]
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
#[cfg(unix)]
use shamir_db::storage::types::RecordKey;
use shamir_db::{ShamirDb, SystemStoreConfig};
#[cfg(unix)]
use shamir_query_builder::ddl::{create_index, Tokenizer};
#[cfg(unix)]
use shamir_query_builder::doc;
#[cfg(unix)]
use shamir_query_types::batch::BatchOp;
#[cfg(unix)]
use shamir_types::codecs::interned::query_value_to_inner;
#[cfg(unix)]
use shamir_types::core::interner::InternerKey;
use shamir_types::types::value::QueryValue;
use tempfile::TempDir;

use crate::replication_seed::codec::{decode, digest};
#[cfg(unix)]
use crate::replication_seed::exporter::{export_open, export_open_with_index_limit, push_index};
use crate::replication_seed::maintenance_open::MaintenanceOpen;
use crate::replication_seed::validate_lengths::validate_lengths;
#[cfg(unix)]
use crate::replication_seed::Manifest;
use crate::replication_seed::{verify, SeedError};

// Red before the prefix preflight: serde receives forged container size hints
// (or EOF instead of the explicit pre-allocation bound error).
#[test]
fn forged_messagepack_lengths_fail_before_serde_allocation() {
    for marker in [0xc6, 0xdb, 0xdd, 0xdf] {
        let bytes = [marker, 0xff, 0xff, 0xff, 0xff];
        let error = decode::<QueryValue>(&bytes).unwrap_err();
        assert!(
            error.to_string().contains("length exceeds remaining bytes"),
            "{error}",
        );
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("manifest.mpack"), bytes).unwrap();
        fs::write(dir.path().join("Complete"), digest(&bytes)).unwrap();
        assert!(verify(dir.path())
            .unwrap_err()
            .to_string()
            .contains("length exceeds remaining bytes"));
    }
    // ByteBuf's binary prefix is nested in the row tuple, not just the root.
    let frame = [0x92, 0xc6, 0xff, 0xff, 0xff, 0xff, 0xc4, 0];
    assert!(
        decode::<(serde_bytes::ByteBuf, serde_bytes::ByteBuf)>(&frame)
            .unwrap_err()
            .to_string()
            .contains("length exceeds remaining bytes")
    );
    assert_eq!(decode::<Vec<u8>>(&[0x92, 1, 2]).unwrap(), vec![1, 2]);
}

#[test]
fn decoder_container_and_depth_caps_have_boundary_controls() {
    let mut allowed = vec![0xdd, 0, 1, 0, 0];
    allowed.resize(5 + 65_536, 0);
    assert_eq!(decode::<Vec<u8>>(&allowed).unwrap().len(), 65_536);
    let mut oversized = vec![0xdd, 0, 1, 0, 1];
    oversized.resize(5 + 65_537, 0);
    assert!(decode::<Vec<u8>>(&oversized)
        .unwrap_err()
        .to_string()
        .contains("container exceeds"));
    // Maps count pairs, not their doubled encoded child count. This must
    // agree with RecordShape's 65,536-key limit before serde sees the map.
    let mut allowed_map = vec![0xdf, 0, 1, 0, 0];
    allowed_map.resize(5 + 2 * 65_536, 0);
    validate_lengths(&allowed_map).unwrap();
    let mut oversized_map = vec![0xdf, 0, 1, 0, 1];
    oversized_map.resize(5 + 2 * 65_537, 0);
    assert!(validate_lengths(&oversized_map)
        .unwrap_err()
        .to_string()
        .contains("container exceeds"));
    let mut nested = vec![0x91; 64];
    nested.push(0);
    assert!(decode::<QueryValue>(&nested)
        .unwrap_err()
        .to_string()
        .contains("depth exceeds"));
}

#[tokio::test]
async fn catalogue_record_count_cap_is_independent_of_byte_cap() {
    let source = ShamirDb::init_memory().await.unwrap();
    // SystemStore::init attaches empty tables; ShamirDb::init_memory only
    // loads their catalogues and does not write default settings/databases.
    // Pin that prerequisite against a future boot-default addition.
    assert_eq!(
        source.system_store().load_databases().await.unwrap().len(),
        0,
    );
    assert_eq!(
        source
            .system_store()
            .load_repositories()
            .await
            .unwrap()
            .len(),
        0,
    );
    assert_eq!(source.system_store().load_tables().await.unwrap().len(), 0);
    assert_eq!(
        source
            .system_store()
            .load_setting("setting-0")
            .await
            .unwrap(),
        None,
    );
    for index in 0..4096 {
        source
            .system_store()
            .save_setting(&format!("setting-{index}"), &QueryValue::Null)
            .await
            .unwrap();
    }
    source.check_seed_catalogue().await.unwrap();
    // Updating an existing setting adds history but no live catalogue row.
    source
        .system_store()
        .save_setting("setting-0", &QueryValue::Str("updated".into()))
        .await
        .unwrap();
    source.check_seed_catalogue().await.unwrap();
    assert_eq!(
        source
            .system_store()
            .load_setting("setting-0")
            .await
            .unwrap(),
        Some(QueryValue::Str("updated".into())),
    );
    source
        .system_store()
        .save_setting("one-too-many", &QueryValue::Null)
        .await
        .unwrap();
    assert!(source
        .check_seed_catalogue()
        .await
        .unwrap_err()
        .to_string()
        .contains("budget"));
}

#[test]
fn maintenance_catalogue_cap_refuses_before_complete_loaders() {
    let dir = TempDir::new().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let meta = dir.path().join("shamir_db_meta.redb");
        let source = ShamirDb::init(SystemStoreConfig::Fjall(meta))
            .await
            .unwrap();
        source.check_seed_catalogue().await.unwrap();
        source
            .system_store()
            .save_setting("oversized", &QueryValue::Str("x".repeat(8 * 1024 * 1024)))
            .await
            .unwrap();
        assert!(source
            .check_seed_catalogue()
            .await
            .unwrap_err()
            .to_string()
            .contains("budget"));
        source.flush_all().await.unwrap();
    });
    drop(rt);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        match MaintenanceOpen::open(dir.path()).await {
            Err(SeedError::Database(error)) => assert!(error.to_string().contains("budget")),
            Err(error) => panic!("unexpected refusal: {error}"),
            Ok(_) => panic!("oversized catalogue opened"),
        }
        // Even an initialization failure releases maintenance ownership.
        assert!(crate::maintenance_lock::acquire(dir.path()).is_ok());
    });
    drop(rt);
}

#[cfg(unix)]
#[tokio::test]
async fn sorted_registry_exports_full_descriptors_and_rows() {
    let source = ShamirDb::init_memory().await.unwrap();
    source.create_db("app").await;
    source
        .add_repo("app", RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    source.add_table("app", "main", "rows", true).await.unwrap();
    let repo = source.get_db("app").unwrap().get_repo("main").unwrap();
    let table = repo.get_table("rows").await.unwrap();
    table
        .create_sorted_index("body_sorted", &["body"])
        .await
        .unwrap();
    let interner = table.interner().get().await.unwrap();
    let payload = query_value_to_inner(&doc! { "body" => "seed row" }, interner)
        .unwrap()
        .to_bytes()
        .unwrap();
    table
        .mvcc_store()
        .unwrap()
        .set_versioned_no_wait(RecordKey::from_slice(b"raw-key"), payload.clone())
        .await
        .unwrap();
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("seed");
    let hash = export_open(&source, "app", "main", &out, 1024)
        .await
        .unwrap();
    let definition = table.sorted_indexes().iter_indexes().pop().unwrap();
    let name = interner
        .get_str(&InternerKey::new(definition.name_interned))
        .unwrap();
    assert_eq!(name.as_ref(), "body_sorted");
    assert_index_artifact(&out, &hash, "sorted", &definition, &payload);
    let manifest: Manifest = decode(&fs::read(out.join("manifest.mpack")).unwrap()).unwrap();
    let exported: SortedIndexDefinition =
        decode(&manifest.tables[0].excluded_indexes[0].1).unwrap();
    let mut bindings = Vec::new();
    for file in &manifest.files {
        if file.path.starts_with("bindings-") {
            let page: Vec<(u64, String)> =
                decode(&fs::read(out.join(&file.path)).unwrap()).unwrap();
            bindings.extend(page);
        }
    }
    assert!(bindings
        .iter()
        .any(|(id, name)| *id == exported.name_interned && name == "body_sorted"));
}

#[cfg(unix)]
#[tokio::test]
async fn index2_registry_exports_full_descriptors_and_rows() {
    let source = ShamirDb::init_memory().await.unwrap();
    source.create_db("app").await;
    source
        .add_repo("app", RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    source
        .add_table("app", "main", "rows", false)
        .await
        .unwrap();
    let repo = source.get_db("app").unwrap().get_repo("main").unwrap();
    let table = repo.get_table("rows").await.unwrap();
    let BatchOp::CreateIndex(op) = create_index("body_fts", "rows")
        .fts(vec!["body".to_string()], Tokenizer::Unicode)
        .unwrap()
    else {
        panic!("builder must produce CreateIndex");
    };
    table.create_index_v2(&op).await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let payload = query_value_to_inner(&doc! { "body" => "seed row" }, interner)
        .unwrap()
        .to_bytes()
        .unwrap();
    table
        .mvcc_store()
        .unwrap()
        .set_versioned_no_wait(RecordKey::from_slice(b"raw-key"), payload.clone())
        .await
        .unwrap();
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("seed");
    let hash = export_open(&source, "app", "main", &out, 1024)
        .await
        .unwrap();
    let definition = table
        .index2_registry()
        .all_descriptors()
        .await
        .pop()
        .unwrap();
    assert_eq!(definition.name, "body_fts");
    assert_index_artifact(&out, &hash, "index2", &definition, &payload);
    let manifest: Manifest = decode(&fs::read(out.join("manifest.mpack")).unwrap()).unwrap();
    let exported: IndexDescriptor = decode(&manifest.tables[0].excluded_indexes[0].1).unwrap();
    assert_eq!(exported.name, "body_fts");
}

#[cfg(unix)]
fn assert_index_artifact<T: serde::Serialize>(
    out: &std::path::Path,
    hash: &str,
    family: &str,
    definition: &T,
    payload: &[u8],
) {
    assert_eq!(verify(out).unwrap(), hash);
    let bytes = fs::read(out.join("manifest.mpack")).unwrap();
    assert_eq!(digest(&bytes), hash);
    let manifest: Manifest = decode(&bytes).unwrap();
    assert_eq!(manifest.index_policy, "EXCLUDED");
    assert_eq!(manifest.tables.len(), 1);
    let table = &manifest.tables[0];
    assert_eq!(table.name, "rows");
    assert_eq!(table.live_records, 1);
    assert_eq!(
        table.excluded_indexes,
        vec![(family.into(), rmp_serde::to_vec_named(definition).unwrap())],
    );
    let frame = rmp_serde::to_vec_named(&(
        serde_bytes::Bytes::new(b"raw-key"),
        serde_bytes::Bytes::new(payload),
    ))
    .unwrap();
    let mut expected = u32::try_from(frame.len()).unwrap().to_be_bytes().to_vec();
    expected.extend_from_slice(&frame);
    let row_files: Vec<_> = manifest
        .files
        .iter()
        .filter(|file| file.table.is_some())
        .collect();
    assert_eq!(row_files.len(), 1);
    assert_eq!(row_files[0].rows, 1);
    assert_eq!(row_files[0].sha256, digest(&expected));
    for file in &manifest.files {
        let bytes = fs::read(out.join(&file.path)).unwrap();
        assert_eq!(file.bytes, bytes.len() as u64);
        assert_eq!(file.sha256, digest(&bytes));
        if file.table.is_some() {
            assert_eq!(bytes, expected);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn descriptor_set_over_budget_refuses_without_truncating() {
    let source = ShamirDb::init_memory().await.unwrap();
    source.create_db("app").await;
    source
        .add_repo("app", RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    source.add_table("app", "main", "rows", true).await.unwrap();
    let repo = source.get_db("app").unwrap().get_repo("main").unwrap();
    let table = repo.get_table("rows").await.unwrap();
    for name in ["first_sorted", "second_sorted"] {
        table.create_sorted_index(name, &["body"]).await.unwrap();
    }
    let definitions = table.sorted_indexes().iter_indexes();
    assert_eq!(definitions.len(), 2);
    let mut entries = Vec::new();
    let mut used = 5;
    push_index(&mut entries, &mut used, 4096, "sorted", &definitions[0]).unwrap();
    let limit = used;
    let before = entries.clone();
    let error = push_index(&mut entries, &mut used, limit, "sorted", &definitions[1]).unwrap_err();
    assert!(error.to_string().contains("budget exceeded"), "{error}");
    assert_eq!(entries, before);
    assert_eq!(used, limit);
    assert_eq!(table.sorted_indexes().iter_indexes().len(), 2);
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("seed");
    let error = export_open_with_index_limit(&source, "app", "main", &out, 1024, limit)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("budget exceeded"), "{error}");
    assert!(!out.exists());
    for entry in fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        assert!(!path.join("manifest.mpack").exists());
        assert!(!path.join("Complete").exists());
        assert!(verify(&path).is_err());
    }
    // A refused descriptor set is not silently published as a partial inventory.
    let hash = export_open(&source, "app", "main", &out, 1024)
        .await
        .unwrap();
    assert_eq!(verify(&out).unwrap(), hash);
    let manifest: Manifest = decode(&fs::read(out.join("manifest.mpack")).unwrap()).unwrap();
    assert_eq!(manifest.tables[0].excluded_indexes.len(), 2);
    let mut expected: Vec<_> = definitions
        .iter()
        .map(|definition| {
            (
                "sorted".to_string(),
                rmp_serde::to_vec_named(definition).unwrap(),
            )
        })
        .collect();
    expected.sort();
    assert_eq!(manifest.tables[0].excluded_indexes, expected);
}
