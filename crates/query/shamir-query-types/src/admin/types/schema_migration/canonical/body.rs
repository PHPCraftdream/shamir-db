//! Schema-migration canonical format v1. All lengths and counts are `u32` big-endian; strings are UTF-8 bytes with no normalization; integers are big-endian; booleans are one byte (`0`/`1`); optional values are `0` or `1 || value`.
//!
//! Body field order: `id:string`, `ordinal:u64`, `display_name:string`, function dependencies, operations. Dependency rows are sorted by function UTF-8 bytes; each dependency-name list is sorted likewise. Operations retain authored order and are encoded as `u32 length || operation bytes`.
//!
//! Each operation begins with `b"shamir-db schema migration op v1\\0" || tag:u16be`. Table tags are create `1`, drop `2`, rename `3`; index tags are create `4`, drop `5`, rename `6`; schema tags are set `7`, add `8`, remove `9`; function tags are create `10`, drop `11`, rename `12`, check `13`, alter `14`; validator tags are create `15`, drop `16`, rename `17`, bind `18`, unbind `19`. DTO fields follow `ddl.rs`. `crate::hmac::canonical_batch_op` supplies v2 bytes for drop table/index/function/validator and create function/validator; custom encoders handle other operations. Exact source/module bytes are appended.
//!
//! Exclude actor/time/connections, request IDs, HMAC tags, journal state; exact code/order are identity; sort maps/sets.
//!
//! Admission remains deliberately narrow: no DML because existing DTOs lack bounded-work receipts/postconditions for uncertainty; Big/Dec/Set because wire DTO conversion is lossy; scalar and dynamic schema expressions because external semantics can change. The closed `SchemaMigrationOp` model stores neither arbitrary callbacks nor the full `BatchOp` enum.

use sha2::{Digest, Sha256};

use super::{ddl, function};
use crate::admin::types::schema_migration::{SchemaMigrationBody, SchemaMigrationOp};

pub(crate) const DOMAIN_SEPARATOR: &[u8] = b"shamir-db schema migration\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaMigrationCanonicalError {
    LengthOverflow,
    UnknownVersion(u16),
    InvalidModel(String),
    ExistingOperationEncoding,
}

impl std::fmt::Display for SchemaMigrationCanonicalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LengthOverflow => f.write_str("schema migration canonical length exceeds u32"),
            Self::UnknownVersion(version) => {
                write!(f, "unknown schema migration canonical version {version}")
            }
            Self::InvalidModel(error) => write!(f, "invalid schema migration body: {error}"),
            Self::ExistingOperationEncoding => {
                f.write_str("existing operation canonicalization rejected an admitted DTO")
            }
        }
    }
}

impl std::error::Error for SchemaMigrationCanonicalError {}

pub(crate) struct Encoder {
    pub(crate) bytes: Vec<u8>,
}

impl Encoder {
    pub(crate) fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub(crate) fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) -> Result<(), SchemaMigrationCanonicalError> {
        let length = u32::try_from(value.len())
            .map_err(|_| SchemaMigrationCanonicalError::LengthOverflow)?;
        self.u32(length);
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    pub(crate) fn str(&mut self, value: &str) -> Result<(), SchemaMigrationCanonicalError> {
        self.bytes(value.as_bytes())
    }

    pub(crate) fn optional<T>(
        &mut self,
        value: Option<&T>,
        encode: impl FnOnce(&mut Self, &T) -> Result<(), SchemaMigrationCanonicalError>,
    ) -> Result<(), SchemaMigrationCanonicalError> {
        match value {
            None => self.u8(0),
            Some(value) => {
                self.u8(1);
                encode(self, value)?;
            }
        }
        Ok(())
    }

    pub(crate) fn strings(
        &mut self,
        values: &[String],
    ) -> Result<(), SchemaMigrationCanonicalError> {
        self.count(values.len())?;
        for value in values {
            self.str(value)?;
        }
        Ok(())
    }

    pub(crate) fn count(&mut self, count: usize) -> Result<(), SchemaMigrationCanonicalError> {
        self.u32(u32::try_from(count).map_err(|_| SchemaMigrationCanonicalError::LengthOverflow)?);
        Ok(())
    }
}

pub fn canonical_body(
    body: &SchemaMigrationBody,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    if body.metadata.canonical_version != 1 {
        return Err(SchemaMigrationCanonicalError::UnknownVersion(
            body.metadata.canonical_version,
        ));
    }
    body.validate()
        .map_err(|error| SchemaMigrationCanonicalError::InvalidModel(error.to_string()))?;
    let mut encoder = Encoder::new();
    encoder.str(&body.metadata.id)?;
    encoder.u64(body.metadata.ordinal);
    encoder.str(&body.metadata.display_name)?;
    let mut dependencies: Vec<_> = body.function_dependencies.iter().collect();
    dependencies.sort_by(|left, right| left.function.as_bytes().cmp(right.function.as_bytes()));
    encoder.count(dependencies.len())?;
    for dependency in dependencies {
        encoder.str(&dependency.function)?;
        let mut names: Vec<&String> = dependency.depends_on.iter().collect();
        names.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        encoder.count(names.len())?;
        for name in names {
            encoder.str(name)?;
        }
    }
    encoder.count(body.operations.len())?;
    for operation in &body.operations {
        encoder.bytes(&operation_bytes(operation)?)?;
    }
    Ok(encoder.bytes)
}

pub fn canonical_body_checksum(
    body: &SchemaMigrationBody,
) -> Result<[u8; 32], SchemaMigrationCanonicalError> {
    let bytes = canonical_body(body)?;
    let mut digest = Sha256::new();
    digest.update(DOMAIN_SEPARATOR);
    digest.update(body.metadata.canonical_version.to_be_bytes());
    digest.update(bytes);
    Ok(digest.finalize().into())
}

fn operation_bytes(
    operation: &SchemaMigrationOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    match operation {
        SchemaMigrationOp::DropTable(value) => {
            function::existing_encoding(crate::batch::BatchOp::DropTable((**value).clone()))
        }
        SchemaMigrationOp::DropIndex(value) => {
            function::existing_encoding(crate::batch::BatchOp::DropIndex((**value).clone()))
        }
        SchemaMigrationOp::CreateFunction(value) => function::encode_create_function(value),
        SchemaMigrationOp::DropFunction(value) => {
            function::existing_encoding(crate::batch::BatchOp::DropFunction((**value).clone()))
        }
        SchemaMigrationOp::CreateValidator(value) => function::encode_create_validator(value),
        SchemaMigrationOp::DropValidator(value) => {
            function::existing_encoding(crate::batch::BatchOp::DropValidator((**value).clone()))
        }
        SchemaMigrationOp::CreateTable(value) => ddl::encode_create_table(value),
        SchemaMigrationOp::RenameTable(value) => ddl::encode_rename_table(value),
        SchemaMigrationOp::CreateIndex(value) => ddl::encode_create_index(value),
        SchemaMigrationOp::RenameIndex(value) => ddl::encode_rename_index(value),
        SchemaMigrationOp::SetTableSchema(value) => ddl::encode_set_schema(value),
        SchemaMigrationOp::AddSchemaRule(value) => ddl::encode_add_schema_rule(value),
        SchemaMigrationOp::RemoveSchemaRule(value) => ddl::encode_remove_schema_rule(value),
        SchemaMigrationOp::RenameFunction(value) => ddl::encode_rename_function(value),
        SchemaMigrationOp::CheckFunction(value) => ddl::encode_check_function(value),
        SchemaMigrationOp::AlterFunction(value) => ddl::encode_alter_function(value),
        SchemaMigrationOp::RenameValidator(value) => ddl::encode_rename_validator(value),
        SchemaMigrationOp::BindValidator(value) => ddl::encode_bind_validator(value),
        SchemaMigrationOp::UnbindValidator(value) => ddl::encode_unbind_validator(value),
    }
}
