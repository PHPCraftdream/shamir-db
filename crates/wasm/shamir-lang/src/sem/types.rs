use std::collections::BTreeMap;

/// Interned semantic type address; identity is type equality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeId(pub u32);

/// Canonical record shape; fields sorted by name in UTF-8 byte order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordShape {
    /// Field names with their field types, sorted by name.
    pub fields: Vec<(String, TypeId)>,
}

impl RecordShape {
    /// Looks up one field type by name via binary search over sorted fields.
    pub fn field(&self, name: &str) -> Option<TypeId> {
        self.fields
            .binary_search_by(|(field_name, _)| field_name.as_str().cmp(name))
            .ok()
            .and_then(|index| self.fields.get(index))
            .map(|(_, ty)| *ty)
    }
}

/// Interned semantic type content; `Error` is the internal suppression type
/// that never comes from source and silences follow-on diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeData {
    I32,
    I64,
    F64,
    Bool,
    String,
    Bytes,
    Null,
    Validation,
    Batch,
    Request,
    Response,
    Optional(TypeId),
    List(TypeId),
    /// Indexes the interner's record table.
    Record(u32),
    /// Internal suppression type; compatible with everything.
    Error,
}

/// Private recursion ceiling for structural type walks.
const MAX_TYPE_TRAVERSAL_DEPTH: u32 = 32;

/// Deterministic interner for semantic types and canonical record shapes.
#[derive(Debug)]
pub struct TypeInterner {
    entries: Vec<TypeData>,
    records: Vec<RecordShape>,
    record_index: BTreeMap<Vec<(String, TypeId)>, TypeId>,
    error_id: Option<TypeId>,
}

impl Default for TypeInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeInterner {
    /// Creates an empty interner.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            records: Vec::new(),
            record_index: BTreeMap::new(),
            error_id: None,
        }
    }

    /// Interns one type and appends its id (records go through `intern_record`).
    pub fn intern(&mut self, data: TypeData) -> TypeId {
        let id = TypeId(self.entries.len() as u32);
        self.entries.push(data);
        id
    }

    /// Interns a canonical record shape, deduplicating identical shapes.
    /// Duplicate field names must be rejected by the caller beforehand.
    pub fn intern_record(&mut self, mut fields: Vec<(String, TypeId)>) -> TypeId {
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(&id) = self.record_index.get(&fields) {
            return id;
        }
        let shape_index = self.records.len() as u32;
        self.records.push(RecordShape {
            fields: fields.clone(),
        });
        let type_id = self.intern(TypeData::Record(shape_index));
        self.record_index.insert(fields, type_id);
        type_id
    }

    /// Copies out the content of one interned type.
    pub fn data(&self, id: TypeId) -> Option<TypeData> {
        self.entries.get(id.0 as usize).copied()
    }

    /// Looks up one record shape by its table index.
    pub fn record(&self, index: u32) -> Option<&RecordShape> {
        self.records.get(index as usize)
    }

    /// Interns the suppression type exactly once and returns its id.
    pub fn error(&mut self) -> TypeId {
        if let Some(id) = self.error_id {
            return id;
        }
        let id = self.intern(TypeData::Error);
        self.error_id = Some(id);
        id
    }

    /// True iff `id` is the suppression type.
    pub fn is_error(&self, id: TypeId) -> bool {
        matches!(self.data(id), Some(TypeData::Error))
    }

    /// True iff `id` is an optional type.
    pub fn is_optional(&self, id: TypeId) -> bool {
        matches!(self.data(id), Some(TypeData::Optional(_)))
    }

    /// The inner type iff `id` is an optional type.
    pub fn underlying_optional(&self, id: TypeId) -> Option<TypeId> {
        match self.data(id) {
            Some(TypeData::Optional(inner)) => Some(inner),
            _ => None,
        }
    }

    /// True iff `id` is the null literal type.
    pub fn is_null_type(&self, id: TypeId) -> bool {
        matches!(self.data(id), Some(TypeData::Null))
    }

    /// True iff `id` is I32, I64 or F64.
    pub fn is_numeric(&self, id: TypeId) -> bool {
        matches!(
            self.data(id),
            Some(TypeData::I32) | Some(TypeData::I64) | Some(TypeData::F64)
        )
    }

    /// True iff `id` is I32 or I64.
    pub fn is_integer(&self, id: TypeId) -> bool {
        matches!(self.data(id), Some(TypeData::I32) | Some(TypeData::I64))
    }

    /// True iff `id` is one of the opaque capability types.
    pub fn is_opaque(&self, id: TypeId) -> bool {
        matches!(
            self.data(id),
            Some(TypeData::Validation)
                | Some(TypeData::Batch)
                | Some(TypeData::Request)
                | Some(TypeData::Response)
        )
    }

    /// True iff equality is defined for `id`; the `Error` suppression type
    /// always reports true. Recursion is depth-guarded.
    pub fn supports_equality(&self, id: TypeId, depth: u32) -> bool {
        if depth > MAX_TYPE_TRAVERSAL_DEPTH {
            return false;
        }
        match self.data(id) {
            Some(
                TypeData::I32
                | TypeData::I64
                | TypeData::F64
                | TypeData::Bool
                | TypeData::String
                | TypeData::Bytes
                | TypeData::Error,
            ) => true,
            Some(TypeData::Optional(inner)) | Some(TypeData::List(inner)) => {
                self.supports_equality(inner, depth.saturating_add(1))
            }
            Some(TypeData::Record(index)) => match self.record(index) {
                Some(shape) => shape.fields.iter().all(|(_, field_ty)| {
                    self.supports_equality(*field_ty, depth.saturating_add(1))
                }),
                None => false,
            },
            _ => false,
        }
    }

    /// True iff `id` can cross the host boundary; the `Error` suppression type
    /// reports false so errors stay localized. Recursion is depth-guarded.
    pub fn wire_safe(&self, id: TypeId, depth: u32) -> bool {
        if depth > MAX_TYPE_TRAVERSAL_DEPTH {
            return false;
        }
        match self.data(id) {
            Some(
                TypeData::I32
                | TypeData::I64
                | TypeData::F64
                | TypeData::Bool
                | TypeData::String
                | TypeData::Bytes,
            ) => true,
            Some(TypeData::Optional(inner)) | Some(TypeData::List(inner)) => {
                self.wire_safe(inner, depth.saturating_add(1))
            }
            Some(TypeData::Record(index)) => match self.record(index) {
                Some(shape) => shape
                    .fields
                    .iter()
                    .all(|(_, field_ty)| self.wire_safe(*field_ty, depth.saturating_add(1))),
                None => false,
            },
            _ => false,
        }
    }
}
