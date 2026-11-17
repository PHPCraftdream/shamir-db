use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

use super::content::{LogicalValue, PortableItem, RecordItem};
use super::header::LogicalDumpHeader;
use super::section::SectionKind;
use crate::replication_seed::artifact::encoding::canonical::{
    decode_canonical_named, encode_canonical_named,
};

/// Encode one canonical named-MessagePack frame payload.
pub fn encode_canonical<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, SeedError> {
    encode_canonical_named(value, 16 * 1024 * 1024)
}

/// Decode strict canonical MessagePack with limits checked before allocation.
pub fn decode_canonical<T: serde::de::DeserializeOwned + serde::Serialize>(
    bytes: &[u8],
) -> Result<T, SeedError> {
    decode_canonical_named(bytes)
}

pub fn encode_header(
    header: &LogicalDumpHeader,
    supported_features: &[&str],
) -> Result<Vec<u8>, SeedError> {
    header.validate(supported_features).map_err(invalid)?;
    encode_canonical(header)
}

pub fn decode_header(
    bytes: &[u8],
    supported_features: &[&str],
) -> Result<LogicalDumpHeader, SeedError> {
    let header: LogicalDumpHeader = decode_canonical(bytes)?;
    header.validate(supported_features).map_err(invalid)?;
    Ok(header)
}

pub fn encode_value(value: &LogicalValue) -> Result<Vec<u8>, SeedError> {
    encode_canonical(value)
}

pub fn decode_value(bytes: &[u8]) -> Result<LogicalValue, SeedError> {
    decode_canonical(bytes)
}

pub fn validate_item_section(section: SectionKind, item: &PortableItem) -> Result<(), SeedError> {
    let valid = matches!(
        (section, item),
        (SectionKind::Identities, PortableItem::Identity(_))
            | (SectionKind::Database, PortableItem::Database(_))
            | (SectionKind::Repositories, PortableItem::Repository(_))
            | (SectionKind::Tables, PortableItem::Table(_))
            | (
                SectionKind::PortableSchemas,
                PortableItem::PortableSchema(_)
            )
            | (
                SectionKind::ValidatorsAndFunctions,
                PortableItem::Validator(_)
            )
            | (
                SectionKind::ValidatorsAndFunctions,
                PortableItem::Function(_)
            )
            | (
                SectionKind::GuestWasmModules,
                PortableItem::GuestWasmModule(_)
            )
            | (SectionKind::Bindings, PortableItem::Binding(_))
            | (
                SectionKind::IndexDescriptors,
                PortableItem::IndexDescriptor(_)
            )
            | (SectionKind::Acls, PortableItem::Acl(_))
            | (SectionKind::Records, PortableItem::Record(_))
    );
    if valid {
        Ok(())
    } else {
        Err(invalid("logical item does not match its section"))
    }
}

/// Convenience constructor keeps record keys as caller-supplied bytes.
pub fn record_item(
    _repository: String,
    _table: String,
    _raw_key: Vec<u8>,
    _value: LogicalValue,
) -> RecordItem {
    RecordItem {
        repository: _repository,
        table: _table,
        raw_key: _raw_key,
        value: _value,
    }
}
