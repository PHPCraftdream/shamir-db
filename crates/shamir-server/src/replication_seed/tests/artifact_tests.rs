use shamir_collections::TMap;
#[cfg(unix)]
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
#[cfg(unix)]
use shamir_db::storage::types::RecordKey;
#[cfg(unix)]
use shamir_db::ShamirDb;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::value::{InnerValue, QueryValue};
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use tempfile::TempDir;

use crate::replication_seed::codec::{decode, digest, encode, referenced};
#[cfg(unix)]
use crate::replication_seed::exporter::export_open;
use crate::replication_seed::schema_as_names::schema_as_names;
#[cfg(unix)]
use crate::replication_seed::{verify, Manifest};

#[cfg(unix)]
async fn fixture() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("app").await;
    db.add_repo("app", RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    db.add_table("app", "main", "empty/../metadata-only", false)
        .await
        .unwrap();
    db.add_table("app", "main", "rows", false).await.unwrap();
    db
}

fn nested_payload(interner: &Interner) -> Vec<u8> {
    let outer = interner.touch_ind("outer").unwrap().into_key();
    let inner = interner.touch_ind("nested").unwrap().into_key();
    let mut child = TMap::default();
    child.insert(inner, InnerValue::Int(9007199254740993));
    let mut record = TMap::default();
    record.insert(outer, InnerValue::List(vec![InnerValue::Map(child)]));
    InnerValue::Map(record).to_bytes().unwrap().to_vec()
}

#[cfg(unix)]
#[tokio::test]
async fn empty_tables_exact_keys_nested_bindings_and_tiny_chunks() {
    let source = fixture().await;
    let repo = source.get_db("app").unwrap().get_repo("main").unwrap();
    let table = repo.get_table("rows").await.unwrap();
    let manager = repo.repo_interner().await.unwrap();
    let interner = manager.get().await.unwrap();
    let payload = nested_payload(interner);
    let mvcc = table.mvcc_store().unwrap();
    let keys = [vec![0, 255, 1, 2], vec![1, 0, 128], vec![2, 255, 255]];
    for key in &keys {
        mvcc.set_versioned_no_wait(
            RecordKey::from_slice(key.as_slice()),
            payload.clone().into(),
        )
        .await
        .unwrap();
    }
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("seed");
    let frame = encode(&(
        serde_bytes::Bytes::new(&keys[0]),
        serde_bytes::Bytes::new(&payload),
    ))
    .unwrap();
    let hash = export_open(&source, "app", "main", &out, frame.len() as u64 + 4)
        .await
        .unwrap();
    assert_eq!(verify(&out).unwrap(), hash);
    let manifest: Manifest = decode(&fs::read(out.join("manifest.mpack")).unwrap()).unwrap();
    assert_eq!(
        encode(&manifest).unwrap(),
        fs::read(out.join("manifest.mpack")).unwrap(),
    );
    assert_eq!(digest(&encode(&manifest).unwrap()), hash);
    assert_eq!(manifest.tables.len(), 2);
    assert_eq!(manifest.tables[0].name, "empty/../metadata-only");
    assert_eq!(manifest.tables[0].live_records, 0);
    assert_eq!(manifest.tables[1].live_records, 3);
    let files: Vec<_> = manifest
        .files
        .iter()
        .filter(|file| file.table.is_some())
        .collect();
    assert_eq!(files.len(), 3);
    let mut observed = Vec::new();
    for file in files {
        let bytes = fs::read(out.join(&file.path)).unwrap();
        let (key, value): (serde_bytes::ByteBuf, serde_bytes::ByteBuf) =
            decode(&bytes[4..]).unwrap();
        observed.push(key.into_vec());
        assert_eq!(value.as_ref(), payload.as_slice());
    }
    assert_eq!(observed, keys);
    assert!(export_open(&source, "app", "main", &out, 1024)
        .await
        .unwrap_err()
        .to_string()
        .contains("exists"));
}

#[cfg(unix)]
#[tokio::test]
async fn verify_rejects_flip_missing_extra_and_interruption() {
    let source = fixture().await;
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("seed");
    export_open(&source, "app", "main", &out, 1024)
        .await
        .unwrap();
    let schema = out.join("schema.mpack");
    let original = fs::read(&schema).unwrap();
    let mut flipped = original.clone();
    flipped[0] ^= 1;
    fs::write(&schema, flipped).unwrap();
    assert!(verify(&out).is_err());
    fs::write(&schema, &original).unwrap();
    assert!(!verify(&out).unwrap().is_empty());
    fs::rename(&schema, dir.path().join("missing")).unwrap();
    assert!(verify(&out).is_err());
    fs::rename(dir.path().join("missing"), &schema).unwrap();
    fs::write(out.join("extra"), b"unexpected").unwrap();
    assert!(verify(&out).unwrap_err().to_string().contains("extra"));
    fs::rename(out.join("extra"), dir.path().join("extra")).unwrap();
    fs::rename(out.join("manifest.mpack"), dir.path().join("manifest")).unwrap();
    assert!(verify(&out).is_err());
    fs::rename(dir.path().join("manifest"), out.join("manifest.mpack")).unwrap();
    fs::rename(out.join("Complete"), dir.path().join("Complete")).unwrap();
    assert!(verify(&out).is_err());
}

#[test]
fn schema_paths_are_names_and_dependencies_refused() {
    let interner = Interner::new();
    let key = interner.touch_ind("name").unwrap().into_key();
    let mut rule = TMap::default();
    rule.insert(
        "path".into(),
        QueryValue::List(vec![QueryValue::Int(key.id() as i64)]),
    );
    rule.insert("type".into(), QueryValue::Str("str".into()));
    let schema = QueryValue::List(vec![QueryValue::Map(rule.clone())]);
    let converted = schema_as_names(&schema, &interner).unwrap();
    assert_eq!(converted[0]["path"][0].as_str(), Some("name"));
    rule.insert("foreign_key".into(), QueryValue::Null);
    assert!(schema_as_names(&QueryValue::List(vec![QueryValue::Map(rule)]), &interner).is_err());
    assert!(schema_as_names(&QueryValue::List(vec![QueryValue::Int(0)]), &interner).is_err());
}

#[test]
fn payload_validation_has_rejection_controls_and_nested_oracle() {
    let interner = Interner::new();
    let bytes = nested_payload(&interner);
    let ids = referenced(&bytes).unwrap();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains_key(&interner.get_ind("outer").unwrap().id()));
    assert!(ids.contains_key(&interner.get_ind("nested").unwrap().id()));
    let mut remap = shamir_collections::TFxMap::default();
    remap.insert(interner.get_ind("outer").unwrap().id(), 100);
    remap.insert(interner.get_ind("nested").unwrap().id(), 200);
    let translated = shamir_tx::remap_inner_value_bytes(bytes.clone().into(), &remap).unwrap();
    let translated_ids = referenced(&translated).unwrap();
    assert_eq!(translated_ids.len(), 2);
    assert!(translated_ids.contains_key(&100));
    assert!(translated_ids.contains_key(&200));
    assert!(referenced(&[0xc1]).is_err());
    let mut duplicates = TMap::default();
    duplicates.insert(InternerKey::new(1), InnerValue::Int(7));
    let one = InnerValue::Map(duplicates).to_bytes().unwrap();
    assert_eq!(one[0], 0x81);
    let mut duplicate_record = vec![0x82];
    duplicate_record.extend_from_slice(&one[1..]);
    duplicate_record.extend_from_slice(&one[1..]);
    assert!(referenced(&duplicate_record)
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
    assert!(referenced(&[0x90]).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(referenced(&trailing).is_err());
    let mut nested = vec![0x91; 100];
    nested.push(0xc0);
    assert!(referenced(&nested).is_err());
    assert_eq!(
        digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn corrupt_missing_bindings_deleted_and_aborted_rows() {
    let source = fixture().await;
    let repo = source.get_db("app").unwrap().get_repo("main").unwrap();
    let table = repo.get_table("rows").await.unwrap();
    let mvcc = table.mvcc_store().unwrap();
    let manager = repo.repo_interner().await.unwrap();
    let payload = nested_payload(manager.get().await.unwrap());
    let key = [3, 4, 5];
    mvcc.set_versioned_no_wait(RecordKey::from_slice(key.as_slice()), payload.into())
        .await
        .unwrap();
    mvcc.delete_versioned_no_wait(RecordKey::from_slice(key.as_slice()))
        .await
        .unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let aborted = gate.assign_next_version();
    gate.abort_unwritten(aborted);
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("deleted");
    export_open(&source, "app", "main", &out, 1024)
        .await
        .unwrap();
    let manifest: Manifest = decode(&fs::read(out.join("manifest.mpack")).unwrap()).unwrap();
    assert!(manifest.tables.iter().all(|table| table.live_records == 0));
    assert_eq!(manifest.version, aborted);
    let mut corrupt = TMap::default();
    corrupt.insert(InternerKey::new(999999), InnerValue::Int(7));
    mvcc.set_versioned_no_wait(
        RecordKey::from_slice(key.as_slice()),
        InnerValue::Map(corrupt).to_bytes().unwrap(),
    )
    .await
    .unwrap();
    assert!(export_open(
        &source,
        "app",
        "main",
        &dir.path().join("bad-binding"),
        1024,
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("binding"));
    mvcc.set_versioned_no_wait(RecordKey::from_slice(key.as_slice()), vec![0xc1].into())
        .await
        .unwrap();
    assert!(
        export_open(&source, "app", "main", &dir.path().join("corrupt"), 1024)
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn allocated_unsettled_version_is_not_a_seed_position() {
    let source = fixture().await;
    let repo = source.get_db("app").unwrap().get_repo("main").unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let version = gate.assign_next_version();
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("seed");
    assert!(export_open(&source, "app", "main", &out, 1024)
        .await
        .unwrap_err()
        .to_string()
        .contains("unsettled"));
    assert!(!out.exists());
    gate.abort_unwritten(version);
    assert_eq!(repo.consistent_version().await.unwrap(), version);
}

#[test]
fn raw_frame_round_trip_corpus_and_truncation() {
    for len in [0, 1, 15, 16, 17, 255, 256, 1024] {
        let key: Vec<u8> = (0..len).map(|n| (n % 256) as u8).collect();
        let payload = [0x81, 0x01, 0x07];
        let encoded = encode(&(
            serde_bytes::Bytes::new(&key),
            serde_bytes::Bytes::new(&payload),
        ))
        .unwrap();
        let (observed, value): (serde_bytes::ByteBuf, serde_bytes::ByteBuf) =
            decode(&encoded).unwrap();
        assert_eq!(observed.as_ref(), key.as_slice());
        assert_eq!(value.as_ref(), payload);
        assert!(decode::<(serde_bytes::ByteBuf, serde_bytes::ByteBuf)>(
            &encoded[..encoded.len() - 1],
        )
        .is_err());
    }
}
