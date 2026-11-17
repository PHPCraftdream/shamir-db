use serde::{Deserialize, Serialize};

use super::definitions::{PortableSchemaItem, SchemaRule};
use super::security::{AclItem, IdentityItem};
use super::system::{DatabaseItem, RepositoryItem, TableItem};
use super::values::RecordItem;

/// Closed typed item union for all data sections; there is no arbitrary
/// metadata bag for credentials, native caches, or replication internals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum PortableItem {
    Identity(IdentityItem),
    Database(DatabaseItem),
    Repository(RepositoryItem),
    Table(TableItem),
    PortableSchema(PortableSchemaItem),
    Validator(ValidatorItem),
    Function(FunctionItem),
    GuestWasmModule(GuestWasmModuleItem),
    Binding(BindingItem),
    IndexDescriptor(IndexDescriptorItem),
    Acl(AclItem),
    Record(RecordItem),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatorItem {
    pub name: String,
    pub kind: ValidatorKind,
    pub source: Option<String>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub mode: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidatorKind {
    Declarative,
    Shamir,
    GuestWasm,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionItem {
    pub name: String,
    pub version: u64,
    pub source: Option<String>,
    pub language: FunctionLanguage,
    #[serde(with = "serde_bytes")]
    pub call_signature: Vec<u8>,
    pub metadata: FunctionMetadata,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub mode: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FunctionLanguage {
    Shamir,
    Rust,
    GuestWasm,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionMetadata {
    pub visibility: Visibility,
    pub security: FunctionSecurity,
    pub secret_grants: Vec<String>,
    pub network_grants: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FunctionSecurity {
    Invoker,
    Definer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestWasmModuleItem {
    pub module_name: String,
    #[serde(with = "serde_bytes")]
    pub wasm_bytes: Vec<u8>,
    pub source: Option<String>,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingItem {
    pub repository: String,
    pub table: String,
    pub function: String,
    pub kind: BindingKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BindingKind {
    Validator,
    Transform,
    ForeignKey,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexDescriptorItem {
    pub repository: String,
    pub table: String,
    pub name: String,
    pub kind: IndexDescriptorKind,
    pub fields: Vec<String>,
    pub unique: bool,
    pub options: IndexOptions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexDescriptorKind {
    Hash,
    Sorted,
    FullText,
    Functional,
    Vector,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexOptions {
    pub language: Option<String>,
    pub stopwords: Vec<String>,
    pub stem: bool,
    pub vector_dimension: Option<u32>,
    pub vector_metric: Option<VectorMetric>,
    pub vector_backend: Option<VectorBackend>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorMetric {
    L2,
    Cosine,
    Dot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorBackend {
    InProcessHnsw,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingReference {
    pub repository: String,
    pub table: String,
    pub schema_rule: Option<SchemaRule>,
}
