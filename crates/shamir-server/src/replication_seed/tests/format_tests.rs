use std::fs;

use shamir_collections::TMap;
use shamir_types::types::value::QueryValue;
use tempfile::TempDir;

use crate::replication_seed::artifact_writer::{publish_manifest, ArtifactWriter};
use crate::replication_seed::codec::{decode, digest, encode};
use crate::replication_seed::schema_digest::schema_digest;
use crate::replication_seed::{verify, Manifest, TableRecord};

fn table(reverse: bool) -> TableRecord {
    let mut rule = TMap::default();
    if reverse {
        rule.insert("type".into(), QueryValue::Str("str".into()));
        rule.insert(
            "path".into(),
            QueryValue::List(vec![QueryValue::Str("name".into())]),
        );
    } else {
        rule.insert(
            "path".into(),
            QueryValue::List(vec![QueryValue::Str("name".into())]),
        );
        rule.insert("type".into(), QueryValue::Str("str".into()));
    }
    TableRecord {
        name: "empty".into(),
        source_enable_indexes: true,
        follower_enable_indexes: false,
        schema_version: 1,
        schema: QueryValue::List(vec![QueryValue::Map(rule)]),
        excluded_indexes: vec![],
        live_records: 0,
    }
}

fn artifact(root: &std::path::Path) -> Manifest {
    fs::create_dir_all(root.join("tables/000000")).unwrap();
    let tables = vec![table(false)];
    let mut writer = ArtifactWriter::new(root.to_path_buf());
    writer
        .file("schema.mpack".into(), &encode(&tables).unwrap(), None, 0)
        .unwrap();
    writer
        .file(
            "bindings-000000.mpack".into(),
            &encode(&vec![(1u64, "name".to_string())]).unwrap(),
            None,
            0,
        )
        .unwrap();
    Manifest {
        format_version: 1,
        codec_version: 1,
        requirements: vec![
            "mvcc".into(),
            "raw-inner-value".into(),
            "schema-names".into(),
        ],
        source_db: "app".into(),
        source_repo: "main".into(),
        lineage_id: [1; 16],
        history_epoch: 2,
        version: 7,
        certified_gap: Some((1, 7)),
        schema_sha256: schema_digest(&tables).unwrap(),
        artifact_id: [3; 16],
        index_policy: "EXCLUDED".into(),
        tables,
        files: writer.files,
    }
}

#[cfg(unix)]
#[test]
fn symlink_artifact_entries_are_refused() {
    let dir = TempDir::new().unwrap();
    let manifest = artifact(dir.path());
    publish(dir.path(), &manifest);
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("file"), b"outside").unwrap();
    std::os::unix::fs::symlink(outside.path().join("file"), dir.path().join("escape")).unwrap();
    assert!(verify(dir.path())
        .unwrap_err()
        .to_string()
        .contains("symlink"));
}

#[test]
fn declared_table_count_is_independently_checked() {
    let dir = TempDir::new().unwrap();
    let mut manifest = artifact(dir.path());
    manifest.tables[0].live_records = 1;
    let schema = encode(&manifest.tables).unwrap();
    fs::write(dir.path().join("schema.mpack"), &schema).unwrap();
    manifest.files[0].bytes = schema.len() as u64;
    manifest.files[0].sha256 = digest(&schema);
    publish(dir.path(), &manifest);
    assert!(verify(dir.path())
        .unwrap_err()
        .to_string()
        .contains("count"));
}

fn publish(root: &std::path::Path, manifest: &Manifest) {
    let bytes = encode(manifest).unwrap();
    fs::write(root.join("manifest.mpack"), &bytes).unwrap();
    fs::write(root.join("Complete"), digest(&bytes)).unwrap();
}

#[test]
fn forged_row_prefixes_are_rejected_with_matching_manifest_hashes() {
    for case in ["outer", "tuple-binary", "record-binary"] {
        let dir = TempDir::new().unwrap();
        let mut manifest = artifact(dir.path());
        let frame = match case {
            "tuple-binary" => vec![0x92, 0xc6, 0xff, 0xff, 0xff, 0xff, 0xc4, 0],
            "record-binary" => {
                // Valid row envelope, but the record map's value is a nested
                // array containing a binary prefix claiming u32::MAX bytes.
                let payload = [0x81, 0x01, 0x91, 0xc6, 0xff, 0xff, 0xff, 0xff];
                encode(&(
                    serde_bytes::Bytes::new(b"key"),
                    serde_bytes::Bytes::new(&payload),
                ))
                .unwrap()
            }
            _ => vec![0x92],
        };
        let len = if case == "outer" {
            16 * 1024 * 1024
        } else {
            u32::try_from(frame.len()).unwrap()
        };
        let mut bytes = len.to_be_bytes().to_vec();
        bytes.extend_from_slice(&frame);
        let path = "tables/000000/rows-000000.mpack";
        fs::write(dir.path().join(path), &bytes).unwrap();
        manifest.tables[0].live_records = 1;
        let schema = encode(&manifest.tables).unwrap();
        fs::write(dir.path().join("schema.mpack"), &schema).unwrap();
        manifest.files[0].bytes = schema.len() as u64;
        manifest.files[0].sha256 = digest(&schema);
        manifest.files.push(crate::replication_seed::FileRecord {
            path: path.into(),
            bytes: bytes.len() as u64,
            sha256: digest(&bytes),
            table: Some(0),
            rows: 1,
        });
        publish(dir.path(), &manifest);
        let error = verify(dir.path()).unwrap_err().to_string();
        assert!(
            error.contains(if case == "outer" {
                "truncated row frame"
            } else {
                "length exceeds remaining bytes"
            }),
            "{error}",
        );
    }
}

#[test]
fn canonical_schema_digest_ignores_map_order_counts_and_index_inventory() {
    let first = table(false);
    let mut second = table(true);
    second.live_records = 999;
    second
        .excluded_indexes
        .push(("unique".into(), vec![1, 2, 3]));
    assert_eq!(
        schema_digest(&[first.clone()]).unwrap(),
        schema_digest(&[second.clone()]).unwrap(),
    );
    second.schema_version += 1;
    assert_ne!(
        schema_digest(&[first]).unwrap(),
        schema_digest(&[second]).unwrap(),
    );
}

#[test]
fn identical_manifest_inputs_have_stable_digest_and_gap_refuses_below_through() {
    let dir = TempDir::new().unwrap();
    let manifest = artifact(dir.path());
    let independent = TempDir::new().unwrap();
    let rebuilt = artifact(independent.path());
    let bytes = encode(&manifest).unwrap();
    assert_eq!(bytes, encode(&rebuilt).unwrap());
    let mut changed = artifact(TempDir::new().unwrap().path());
    changed.history_epoch += 1;
    assert_ne!(digest(&bytes), digest(&encode(&changed).unwrap()));
    let decoded: Manifest = decode(&bytes).unwrap();
    assert_eq!(digest(&bytes), digest(&encode(&decoded).unwrap()));
    publish(dir.path(), &manifest);
    assert_eq!(verify(dir.path()).unwrap(), digest(&bytes));
    let mut below = manifest;
    below.version = 6;
    publish(dir.path(), &below);
    assert!(verify(dir.path()).unwrap_err().to_string().contains("gap"));
}

#[test]
fn interrupted_writer_before_manifest_never_verifies() {
    let dir = TempDir::new().unwrap();
    let manifest = artifact(dir.path());
    let bytes = encode(&manifest).unwrap();
    let error = publish_manifest(dir.path(), &bytes, &digest(&bytes), || {
        Err(crate::replication_seed::SeedError::Invalid(
            "injected before-manifest failure".into(),
        ))
    })
    .unwrap_err();
    assert!(error.to_string().contains("injected"));
    assert!(dir.path().join("schema.mpack").is_file());
    assert!(!dir.path().join("manifest.mpack").exists());
    assert!(!dir.path().join("Complete").exists());
    assert!(verify(dir.path()).is_err());
}

#[test]
fn bounded_serialization_rejects_before_growing_past_limit() {
    let input = vec![7u8; 1024];
    let encoded = encode(&input).unwrap();
    assert_eq!(
        crate::replication_seed::encode_bounded::encode_bounded(&input, encoded.len()).unwrap(),
        encoded,
    );
    assert!(
        crate::replication_seed::encode_bounded::encode_bounded(&input, encoded.len() - 1,)
            .is_err()
    );
}

#[test]
fn index_metadata_budget_includes_entry_overhead_before_retaining_entries() {
    let definition = vec![7u8; 8];
    let payload = encode(&definition).unwrap();
    let entry = ("regular".to_string(), payload.clone());
    let entry_size = encode(&entry).unwrap().len() as u64;
    let mut entries = Vec::new();
    let mut used = 5u64;
    let limit = used + entry_size;
    crate::replication_seed::exporter::push_index(
        &mut entries,
        &mut used,
        limit,
        "regular",
        &definition,
    )
    .unwrap();
    assert_eq!(entries, vec![entry]);
    assert_eq!(used, limit);
    assert!(crate::replication_seed::exporter::push_index(
        &mut entries,
        &mut used,
        limit,
        "regular",
        &definition,
    )
    .is_err());
    assert_eq!(entries.len(), 1);
    assert_eq!(used, limit);

    let mut empty = Vec::new();
    let mut empty_used = 5u64;
    // Payload alone fits, but the tuple/family encoding does not.
    assert!(crate::replication_seed::exporter::push_index(
        &mut empty,
        &mut empty_used,
        5 + payload.len() as u64,
        "regular",
        &definition,
    )
    .is_err());
    assert!(empty.is_empty());
    assert_eq!(empty_used, 5);
}

#[test]
fn file_count_and_metadata_budgets_refuse_before_write_or_inventory_growth() {
    let dir = TempDir::new().unwrap();
    let mut count_limited = ArtifactWriter::with_limits(dir.path().to_path_buf(), 1, 4096);
    count_limited
        .file("first".into(), b"first", None, 0)
        .unwrap();
    let error = count_limited
        .file("second".into(), b"second", None, 0)
        .unwrap_err();
    assert!(error.to_string().contains("file count"));
    assert_eq!(count_limited.files.len(), 1);
    assert!(!dir.path().join("second").exists());

    let entry_size = encode(&count_limited.files[0]).unwrap().len() as u64;
    let mut metadata_limited =
        ArtifactWriter::with_limits(dir.path().to_path_buf(), 10, 5 + entry_size);
    metadata_limited
        .file("third".into(), b"third", None, 0)
        .unwrap();
    let error = metadata_limited
        .file("fourth".into(), b"fourth", None, 0)
        .unwrap_err();
    assert!(error.to_string().contains("metadata"));
    assert_eq!(metadata_limited.files.len(), 1);
    assert!(!dir.path().join("fourth").exists());
}

#[test]
fn conflicting_binding_and_unsafe_file_path_fail_even_with_matching_digest() {
    let dir = TempDir::new().unwrap();
    let mut manifest = artifact(dir.path());
    let page = encode(&vec![(1u64, "name".to_string()), (2, "name".to_string())]).unwrap();
    fs::write(dir.path().join("bindings-000000.mpack"), &page).unwrap();
    manifest.files[1].bytes = page.len() as u64;
    manifest.files[1].sha256 = digest(&page);
    publish(dir.path(), &manifest);
    assert!(verify(dir.path())
        .unwrap_err()
        .to_string()
        .contains("binding"));
    manifest.files[1].path = "../outside".into();
    publish(dir.path(), &manifest);
    assert!(verify(dir.path()).unwrap_err().to_string().contains("path"));
}
