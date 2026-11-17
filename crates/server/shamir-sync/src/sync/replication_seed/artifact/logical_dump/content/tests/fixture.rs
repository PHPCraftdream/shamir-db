//! Deterministic logical-dump reference fixture shared by the artifact,
//! corruption and canonical-value tests.

use std::io::Cursor;

use super::super::super::codec::encode_canonical;
use super::super::super::content::{
    AclAction, AclGrant, AclItem, BindingItem, BindingKind, DatabaseItem, FunctionItem,
    FunctionLanguage, FunctionMetadata, FunctionSecurity, GuestWasmModuleItem, IdentityItem,
    IdentityKind, IndexDescriptorItem, IndexDescriptorKind, IndexOptions, LogicalValue,
    PortableItem, PortableSchemaItem, RecordItem, RepositoryEngine, RepositoryItem, ResourceRef,
    SchemaRule, SchemaValueKind, SettingItem, SettingKey, TableItem, ValidatorItem, ValidatorKind,
    VectorBackend, VectorMetric, Visibility,
};
use super::super::super::header::{
    DumpScope, EncryptionMode, LogicalDumpHeader, RepositoryVersion,
};
use super::super::super::section::{SectionInventory, SectionKind};
use crate::replication_seed::artifact::logical_dump::stream::{DumpReader, DumpWriter};

/// Per-section expected counts of the reference fixture (12 sections).
pub const SECTION_COUNTS: [(SectionKind, u64); 12] = [
    (SectionKind::Identities, 1),
    (SectionKind::Database, 1),
    (SectionKind::Repositories, 1),
    (SectionKind::Tables, 1),
    (SectionKind::PortableSchemas, 1),
    (SectionKind::ValidatorsAndFunctions, 2),
    (SectionKind::GuestWasmModules, 1),
    (SectionKind::Bindings, 1),
    (SectionKind::IndexDescriptors, 1),
    (SectionKind::Acls, 1),
    (SectionKind::Records, 2),
    (SectionKind::End, 0),
];

/// Fixed snapshot instant; the fixture must stay byte-deterministic.
pub const SNAPSHOT_INSTANT: u64 = 1_700_000_000;

/// Header matching the fixture sections. Required features are empty because
/// writer and reader validate against the crate's supported-feature list.
pub fn fixture_header() -> LogicalDumpHeader {
    LogicalDumpHeader {
        format_version: 1,
        required_features: Vec::new(),
        source_storage_epoch: Some(7),
        application_schema_version: None,
        snapshot_instant: SNAPSHOT_INSTANT,
        repository_versions: vec![RepositoryVersion {
            repository: "app/main".into(),
            version: 3,
        }],
        scope: DumpScope {
            repository: None,
            table: None,
            schema_only: false,
            include_artifacts: false,
        },
        encryption: EncryptionMode::None,
        sections: SECTION_COUNTS
            .iter()
            .map(|(section, count)| SectionInventory {
                section: *section,
                expected_items: Some(*count),
            })
            .collect(),
    }
}

/// Every `LogicalValue` kind at least once, including signed-zero f64 bits,
/// a NaN payload and insertion-ordered containers.
pub fn covered_value() -> LogicalValue {
    LogicalValue::Map(vec![
        ("n".into(), LogicalValue::Null),
        ("b".into(), LogicalValue::Bool(false)),
        ("i".into(), LogicalValue::Int(i64::MIN)),
        ("z".into(), LogicalValue::F64(0x8000_0000_0000_0000)),
        ("q".into(), LogicalValue::F64(0x7ff8_0000_0000_0042)),
        (
            "d".into(),
            LogicalValue::Decimal {
                coefficient: (1_u128 << 95) + 17,
                scale: 7,
                negative: true,
            },
        ),
        (
            "g".into(),
            LogicalValue::Big("184467440737095516160000000000000000001".into()),
        ),
        (
            "s".into(),
            LogicalValue::Str("184467440737095516160000000000000000001".into()),
        ),
        ("x".into(), LogicalValue::Bin(vec![0, 0xff, 0x7f])),
        (
            "l".into(),
            LogicalValue::List(vec![LogicalValue::Null, LogicalValue::Int(1)]),
        ),
        (
            "e".into(),
            LogicalValue::Set(vec![
                LogicalValue::Str("second".into()),
                LogicalValue::Str("first".into()),
            ]),
        ),
        (
            "m".into(),
            LogicalValue::Map(vec![
                ("z".into(), LogicalValue::Int(1)),
                ("a".into(), LogicalValue::Int(2)),
            ]),
        ),
    ])
}

/// One item of every `PortableItem` variant, in section order.
pub fn fixture_items() -> Vec<(SectionKind, Vec<PortableItem>)> {
    let schema_rule = SchemaRule {
        path: vec!["name".into()],
        kind: SchemaValueKind::Str,
        required: true,
        nullable: false,
        unsigned: false,
        min: None,
        max: None,
        len: None,
        max_len: Some(64),
        min_len: Some(1),
        one_of: Vec::new(),
        default: None,
        array_of: None,
        scalar: None,
        format: None,
        compare: None,
        foreign_key: None,
        unique: false,
        auto_now: false,
        auto_now_add: false,
        keyset_safe: false,
    };
    grouped(vec![
        (
            SectionKind::Identities,
            PortableItem::Identity(IdentityItem {
                logical_name: "svc".into(),
                kind: IdentityKind::User,
                disabled_on_import: true,
            }),
        ),
        (
            SectionKind::Database,
            PortableItem::Database(DatabaseItem {
                name: "app".into(),
                owner: Some("owner".into()),
                group: None,
                mode: 0o644,
                settings: vec![SettingItem {
                    key: SettingKey::RootMeta,
                    value: LogicalValue::Decimal {
                        coefficient: 1,
                        scale: 2,
                        negative: true,
                    },
                }],
            }),
        ),
        (
            SectionKind::Repositories,
            PortableItem::Repository(RepositoryItem {
                name: "app/main".into(),
                engine: RepositoryEngine::Hybrid,
                durability: crate::replication_seed::Durability::Durable,
                owner: None,
                group: None,
                mode: 0o600,
            }),
        ),
        (
            SectionKind::Tables,
            PortableItem::Table(TableItem {
                repository: "app/main".into(),
                name: "accounts".into(),
                enable_indexes: true,
                schema_version: 2,
                schema: LogicalValue::Map(vec![
                    ("z".into(), LogicalValue::Str("last".into())),
                    ("a".into(), LogicalValue::Str("first".into())),
                ]),
                schema_validator: Some("v1".into()),
                buffer_ttl_ms: Some(5_000),
                owner: None,
                group: None,
                mode: 0o600,
            }),
        ),
        (
            SectionKind::PortableSchemas,
            PortableItem::PortableSchema(PortableSchemaItem {
                repository: "app/main".into(),
                table: "accounts".into(),
                rules: vec![schema_rule],
            }),
        ),
        // Canonical order is by (name, kind tag): "f1" < "v1", so Function first.
        (
            SectionKind::ValidatorsAndFunctions,
            PortableItem::Function(FunctionItem {
                name: "f1".into(),
                version: 1,
                source: Some("fn main() {}".into()),
                language: FunctionLanguage::Shamir,
                call_signature: vec![1, 2, 3],
                metadata: FunctionMetadata {
                    visibility: Visibility::Public,
                    security: FunctionSecurity::Invoker,
                    secret_grants: Vec::new(),
                    network_grants: Vec::new(),
                },
                owner: None,
                group: None,
                mode: 0o644,
            }),
        ),
        (
            SectionKind::ValidatorsAndFunctions,
            PortableItem::Validator(ValidatorItem {
                name: "v1".into(),
                kind: ValidatorKind::Declarative,
                source: None,
                owner: None,
                group: None,
                mode: 0o644,
            }),
        ),
        (
            SectionKind::GuestWasmModules,
            PortableItem::GuestWasmModule(GuestWasmModuleItem {
                module_name: "m".into(),
                wasm_bytes: vec![0, 1, 159, 146, 150],
                source: None,
                required: false,
            }),
        ),
        (
            SectionKind::Bindings,
            PortableItem::Binding(BindingItem {
                repository: "app/main".into(),
                table: "accounts".into(),
                function: "f1".into(),
                kind: BindingKind::Transform,
            }),
        ),
        (
            SectionKind::IndexDescriptors,
            PortableItem::IndexDescriptor(IndexDescriptorItem {
                repository: "app/main".into(),
                table: "accounts".into(),
                name: "by_name".into(),
                kind: IndexDescriptorKind::Sorted,
                fields: vec!["name".into()],
                unique: false,
                options: IndexOptions {
                    language: None,
                    stopwords: Vec::new(),
                    stem: false,
                    vector_dimension: Some(3),
                    vector_metric: Some(VectorMetric::Cosine),
                    vector_backend: Some(VectorBackend::InProcessHnsw),
                },
            }),
        ),
        (
            SectionKind::Acls,
            PortableItem::Acl(AclItem {
                resource: ResourceRef {
                    database: "app".into(),
                    repository: Some("app/main".into()),
                    table: Some("accounts".into()),
                    object: None,
                },
                owner: None,
                group: None,
                mode: 0o640,
                grants: vec![AclGrant {
                    principal: "svc".into(),
                    principal_kind: IdentityKind::User,
                    actions: vec![AclAction::Read, AclAction::List],
                }],
            }),
        ),
        (
            SectionKind::Records,
            PortableItem::Record(RecordItem {
                repository: "app/main".into(),
                table: "accounts".into(),
                raw_key: vec![0x01],
                value: covered_value(),
            }),
        ),
        (
            SectionKind::Records,
            PortableItem::Record(RecordItem {
                repository: "app/main".into(),
                table: "accounts".into(),
                raw_key: vec![0x02],
                value: LogicalValue::Set(vec![LogicalValue::Int(2), LogicalValue::Int(1)]),
            }),
        ),
    ])
}

/// Group consecutive same-section items so multi-item sections stay together.
fn grouped(items: Vec<(SectionKind, PortableItem)>) -> Vec<(SectionKind, Vec<PortableItem>)> {
    let mut sections: Vec<(SectionKind, Vec<PortableItem>)> = Vec::new();
    for (section, item) in items {
        match sections.last_mut() {
            Some((open, group)) if *open == section => group.push(item),
            _ => sections.push((section, vec![item])),
        }
    }
    sections
}

/// Serialize the full deterministic reference artifact into memory.
pub fn build_fixture() -> Vec<u8> {
    let mut bytes = Vec::new();
    let writer = DumpWriter::new(&mut bytes, fixture_header()).expect("fixture preamble writes");
    let writer = write_all_sections(writer);
    writer.finish().expect("fixture trailer writes");
    bytes
}

fn write_all_sections<W: std::io::Write>(mut writer: DumpWriter<W>) -> DumpWriter<W> {
    for (section, items) in fixture_items() {
        writer.start_section(section).expect("section starts");
        for item in items {
            writer.write_item(&item).expect("item writes");
        }
        writer.end_section().expect("section ends");
    }
    writer
}

/// Round-trip the fixture through the reader and return the items in order.
pub fn read_fixture_items(bytes: &[u8]) -> (LogicalDumpHeader, Vec<PortableItem>) {
    let mut reader = DumpReader::new(Cursor::new(bytes)).expect("fixture opens");
    let header = reader.header().clone();
    let mut items = Vec::new();
    while let Some(item) = reader.next_item().expect("fixture reads") {
        items.push(item);
    }
    assert!(reader.finished, "fixture reader reached verified EOF");
    (header, items)
}

/// Frame boundary info for corruption tests.
pub struct FrameSpan {
    pub start: usize,
    pub wire_len: usize,
}

/// Walk every frame of a fixture artifact (16-byte preamble + frames).
pub fn walk_frames(bytes: &[u8]) -> (usize, Vec<FrameSpan>) {
    let header_length =
        u32::from_be_bytes(bytes[12..16].try_into().expect("header length")) as usize;
    let mut offset = 16 + header_length;
    let mut frames = Vec::new();
    while offset < bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("frame length"))
            as usize;
        let wire_len = 4 + length + 32;
        frames.push(FrameSpan {
            start: offset,
            wire_len,
        });
        offset += wire_len;
    }
    (16 + header_length, frames)
}

/// Canonical payload of a section-item record, as the writer produces it.
pub fn item_record_payload(item: &PortableItem) -> Vec<u8> {
    encode_canonical(&super::super::super::section::SectionRecord::Item {
        section: SectionKind::Identities,
        item: item.clone(),
    })
    .expect("item record encodes")
}
