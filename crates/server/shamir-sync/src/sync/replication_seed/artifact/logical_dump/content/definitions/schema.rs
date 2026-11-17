use serde::{Deserialize, Serialize};

use super::super::values::LogicalValue;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSchemaItem {
    pub repository: String,
    pub table: String,
    pub rules: Vec<SchemaRule>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SchemaValueKind {
    Null,
    Bool,
    Int,
    F64,
    Dec,
    Big,
    Str,
    Bin,
    List,
    Set,
    Map,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaRule {
    pub path: Vec<String>,
    pub kind: SchemaValueKind,
    pub required: bool,
    pub nullable: bool,
    pub unsigned: bool,
    pub min: Option<SchemaNumber>,
    pub max: Option<SchemaNumber>,
    pub len: Option<u64>,
    pub max_len: Option<u64>,
    pub min_len: Option<u64>,
    pub one_of: Vec<LogicalValue>,
    pub default: Option<LogicalValue>,
    pub array_of: Option<SchemaValueKind>,
    pub scalar: Option<String>,
    pub format: Option<String>,
    pub compare: Option<SchemaCompare>,
    pub foreign_key: Option<ForeignKeyRule>,
    pub unique: bool,
    pub auto_now: bool,
    pub auto_now_add: bool,
    pub keyset_safe: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", deny_unknown_fields)]
pub enum SchemaNumber {
    Int(i64),
    F64(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaCompare {
    pub other: Vec<String>,
    pub op: CompareOperator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompareOperator {
    Less,
    LessEqual,
    Equal,
    NotEqual,
    GreaterEqual,
    Greater,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForeignKeyRule {
    pub ref_table: String,
    pub ref_field: String,
    pub on_delete: ForeignKeyAction,
    pub on_update: ForeignKeyAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForeignKeyAction {
    NoAction,
    Restrict,
    Cascade,
    SetNull,
}
