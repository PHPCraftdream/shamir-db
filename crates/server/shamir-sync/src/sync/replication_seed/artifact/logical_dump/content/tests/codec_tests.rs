use shamir_types::types::common::new_map;

use super::super::super::codec::{
    decode_canonical, decode_header, decode_value, encode_canonical, encode_header, encode_value,
    validate_item_section,
};
use super::super::super::content::{LogicalValue, PortableItem, RecordItem};
use super::super::super::header::{
    DumpScope, EncryptionMode, LogicalDumpHeader, RepositoryVersion,
};
use super::super::super::section::{SectionInventory, SectionKind};

#[test]
fn logical_values_round_trip_all_kinds_and_preserve_container_order() {
    let values = [
        LogicalValue::Null,
        LogicalValue::Bool(true),
        LogicalValue::Int(i64::MIN),
        LogicalValue::F64(0x8000_0000_0000_0000),
        LogicalValue::F64(0x7ff8_0000_0000_0042),
        LogicalValue::Decimal {
            coefficient: (1_u128 << 95) + 17,
            scale: 7,
            negative: true,
        },
        LogicalValue::Big("184467440737095516160000000000000000001".into()),
        LogicalValue::Str("184467440737095516160000000000000000001".into()),
        LogicalValue::Bin(vec![0, 0xff, 0x7f]),
        LogicalValue::List(vec![LogicalValue::Null, LogicalValue::Int(1)]),
        LogicalValue::Set(vec![
            LogicalValue::Str("second".into()),
            LogicalValue::Str("first".into()),
        ]),
        LogicalValue::Map(vec![
            ("z".into(), LogicalValue::Int(1)),
            ("a".into(), LogicalValue::Int(2)),
        ]),
    ];
    for value in values {
        let bytes = encode_value(&value).expect("logical value encodes");
        let decoded = decode_value(&bytes).expect("logical value decodes");
        assert_eq!(decoded, value);
    }
}

#[test]
fn canonical_fields_are_sorted_and_invalid_encodings_rejected() {
    let mut fields = new_map();
    fields.insert("z".to_owned(), 1_u8);
    fields.insert("a".to_owned(), 2_u8);
    let encoded = encode_canonical(&fields).expect("named map encodes");
    let decoded: shamir_types::types::common::TMap<String, u8> =
        decode_canonical(&encoded).expect("map decodes");
    assert_eq!(
        decoded.keys().map(String::as_str).collect::<Vec<_>>(),
        ["a", "z"]
    );

    let unsorted = [0x82, 0xa1, b'z', 1, 0xa1, b'a', 2];
    let duplicate = [0x82, 0xa1, b'a', 1, 0xa1, b'a', 2];
    let invalid_utf8 = [0x81, 0xa1, 0xff, 1];
    assert!(decode_canonical::<serde_json::Value>(&unsorted).is_err());
    assert!(decode_canonical::<serde_json::Value>(&duplicate).is_err());
    assert!(decode_canonical::<serde_json::Value>(&invalid_utf8).is_err());
}

#[test]
fn header_and_section_typing_fail_closed() {
    let header = LogicalDumpHeader {
        format_version: 1,
        required_features: vec!["base".into()],
        source_storage_epoch: Some(1),
        application_schema_version: None,
        snapshot_instant: 9,
        repository_versions: vec![RepositoryVersion {
            repository: "app/main".into(),
            version: 4,
        }],
        scope: DumpScope {
            repository: None,
            table: None,
            schema_only: false,
            include_artifacts: false,
        },
        encryption: EncryptionMode::None,
        sections: SectionKind::ORDERED
            .into_iter()
            .map(|section| SectionInventory {
                section,
                expected_items: Some(0),
            })
            .collect(),
    };
    let bytes = encode_header(&header, &["base"]).expect("supported header encodes");
    assert_eq!(
        decode_header(&bytes, &["base"]).expect("header decodes"),
        header
    );
    assert!(decode_header(&bytes, &[]).is_err());

    let item = PortableItem::Record(RecordItem {
        repository: "main".into(),
        table: "items".into(),
        raw_key: vec![0xff],
        value: LogicalValue::Null,
    });
    assert!(validate_item_section(SectionKind::Records, &item).is_ok());
    assert!(validate_item_section(SectionKind::Tables, &item).is_err());
}
