mod artifact;
mod error;
mod export;
mod install;
mod verify;

use artifact::{
    artifact_writer, codec, encode_bounded, file_record, logical_dump, manifest, record_shape,
    table_record, validate_lengths,
};
use export::{exporter, maintenance_open};
pub(crate) use export::{schema_as_names, schema_digest};
use verify::verifier;

pub use error::SeedError;
pub use export::maintenance_open::MaintenanceOpen;
pub use exporter::export;
pub use file_record::FileRecord;
pub use install::{install, install_subscription, install_with, SeedSubscriptionArgs};
pub use logical_dump::stream::{
    preflight_artifact_size, verify_dump, verify_dump_file, DumpReader, DumpReport, DumpWriter,
};
pub use logical_dump::{
    decode_canonical, decode_header, decode_value, encode_canonical, encode_header, encode_value,
    record_item, validate_item_section, AclAction, AclGrant, AclItem, BindingItem, BindingKind,
    BindingReference, CompareOperator, DatabaseItem, DumpScope, Durability, EncryptionMode,
    ForeignKeyAction, ForeignKeyRule, FunctionItem, FunctionLanguage, FunctionMetadata,
    FunctionSecurity, GuestWasmModuleItem, IdentityItem, IdentityKind, IndexDescriptorItem,
    IndexDescriptorKind, IndexOptions, LogicalDumpHeader, LogicalValue, PortableItem,
    PortableSchemaItem, RecordItem, RepositoryEngine, RepositoryItem, RepositoryVersion,
    ResourceRef, SchemaCompare, SchemaNumber, SchemaRule, SchemaValueKind, SectionInventory,
    SectionKind, SectionRecord, SettingItem, SettingKey, TableItem, Trailer, TrailerSection,
    ValidatorItem, ValidatorKind, VectorBackend, VectorMetric, Visibility,
};
pub use manifest::Manifest;
pub use table_record::TableRecord;
pub use verifier::verify;

#[cfg(test)]
mod tests;
