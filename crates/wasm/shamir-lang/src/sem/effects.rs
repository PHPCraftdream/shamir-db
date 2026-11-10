/// Builtin function identity; never represented as strings at use sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltinId {
    Len,
    IsNull,
    Unwrap,
    ToI64,
    ToF64I32,
    ToF64I64,
    ToI32,
    ToI32F64,
    ToI64F64,
    Accept,
    Reject,
    RejectPath,
    RecordError,
    WithError,
    WithRecordError,
    Stop,
    DbGet,
    DbInsert,
    DbQuery,
    Batch,
    BatchId,
    BatchRead,
    BatchInsert,
    Request,
    DbExecute,
    ResponseRows,
    ResponseInserted,
    BatchUpdate,
    BatchDelete,
    ResponseAffected,
}

/// Classification driving capability checks and effect recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinCategory {
    /// Pure computation with no effects.
    Pure,
    /// Validator verdict constructor.
    ValidatorCtor,
    /// Stage-2 database builtins; allowed for procedure entries and helpers.
    Stage2,
}

/// Number of reserved builtin names; kept in sync with `BUILTIN_TABLE`.
pub const BUILTIN_NAME_COUNT: usize = 30;

/// Private const-ordered (name, identity) table scanned linearly.
const BUILTIN_TABLE: [(&str, BuiltinId); BUILTIN_NAME_COUNT] = [
    ("len", BuiltinId::Len),
    ("is_null", BuiltinId::IsNull),
    ("unwrap", BuiltinId::Unwrap),
    ("to_i64", BuiltinId::ToI64),
    ("to_f64_i32", BuiltinId::ToF64I32),
    ("to_f64_i64", BuiltinId::ToF64I64),
    ("to_i32", BuiltinId::ToI32),
    ("to_i32_f64", BuiltinId::ToI32F64),
    ("to_i64_f64", BuiltinId::ToI64F64),
    ("accept", BuiltinId::Accept),
    ("reject", BuiltinId::Reject),
    ("reject_path", BuiltinId::RejectPath),
    ("record_error", BuiltinId::RecordError),
    ("with_error", BuiltinId::WithError),
    ("with_record_error", BuiltinId::WithRecordError),
    ("stop", BuiltinId::Stop),
    ("db_get", BuiltinId::DbGet),
    ("db_insert", BuiltinId::DbInsert),
    ("db_query", BuiltinId::DbQuery),
    ("batch", BuiltinId::Batch),
    ("batch_id", BuiltinId::BatchId),
    ("batch_read", BuiltinId::BatchRead),
    ("batch_insert", BuiltinId::BatchInsert),
    ("request", BuiltinId::Request),
    ("db_execute", BuiltinId::DbExecute),
    ("response_rows", BuiltinId::ResponseRows),
    ("response_inserted", BuiltinId::ResponseInserted),
    ("batch_update", BuiltinId::BatchUpdate),
    ("batch_delete", BuiltinId::BatchDelete),
    ("response_affected", BuiltinId::ResponseAffected),
];

/// The exact source name of one builtin.
pub fn builtin_name(id: BuiltinId) -> &'static str {
    match id {
        BuiltinId::Len => "len",
        BuiltinId::IsNull => "is_null",
        BuiltinId::Unwrap => "unwrap",
        BuiltinId::ToI64 => "to_i64",
        BuiltinId::ToF64I32 => "to_f64_i32",
        BuiltinId::ToF64I64 => "to_f64_i64",
        BuiltinId::ToI32 => "to_i32",
        BuiltinId::ToI32F64 => "to_i32_f64",
        BuiltinId::ToI64F64 => "to_i64_f64",
        BuiltinId::Accept => "accept",
        BuiltinId::Reject => "reject",
        BuiltinId::RejectPath => "reject_path",
        BuiltinId::RecordError => "record_error",
        BuiltinId::WithError => "with_error",
        BuiltinId::WithRecordError => "with_record_error",
        BuiltinId::Stop => "stop",
        BuiltinId::DbGet => "db_get",
        BuiltinId::DbInsert => "db_insert",
        BuiltinId::DbQuery => "db_query",
        BuiltinId::Batch => "batch",
        BuiltinId::BatchId => "batch_id",
        BuiltinId::BatchRead => "batch_read",
        BuiltinId::BatchInsert => "batch_insert",
        BuiltinId::Request => "request",
        BuiltinId::DbExecute => "db_execute",
        BuiltinId::ResponseRows => "response_rows",
        BuiltinId::ResponseInserted => "response_inserted",
        BuiltinId::BatchUpdate => "batch_update",
        BuiltinId::BatchDelete => "batch_delete",
        BuiltinId::ResponseAffected => "response_affected",
    }
}

/// The category of one builtin.
pub fn builtin_category(id: BuiltinId) -> BuiltinCategory {
    match id {
        BuiltinId::Len
        | BuiltinId::IsNull
        | BuiltinId::Unwrap
        | BuiltinId::ToI64
        | BuiltinId::ToF64I32
        | BuiltinId::ToF64I64
        | BuiltinId::ToI32
        | BuiltinId::ToI32F64
        | BuiltinId::ToI64F64 => BuiltinCategory::Pure,
        BuiltinId::Accept
        | BuiltinId::Reject
        | BuiltinId::RejectPath
        | BuiltinId::RecordError
        | BuiltinId::WithError
        | BuiltinId::WithRecordError
        | BuiltinId::Stop => BuiltinCategory::ValidatorCtor,
        BuiltinId::DbGet
        | BuiltinId::DbInsert
        | BuiltinId::DbQuery
        | BuiltinId::Batch
        | BuiltinId::BatchId
        | BuiltinId::BatchRead
        | BuiltinId::BatchInsert
        | BuiltinId::Request
        | BuiltinId::DbExecute
        | BuiltinId::ResponseRows
        | BuiltinId::ResponseInserted
        | BuiltinId::BatchUpdate
        | BuiltinId::BatchDelete
        | BuiltinId::ResponseAffected => BuiltinCategory::Stage2,
    }
}

/// Resolves one source name to its builtin identity, deterministically.
pub fn builtin_by_name(name: &str) -> Option<BuiltinId> {
    BUILTIN_TABLE
        .iter()
        .find(|(table_name, _)| *table_name == name)
        .map(|(_, id)| *id)
}

/// Compile-stage capability ceiling for builtin use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    /// Pure scalar functions.
    Scalar,
    /// Validators additionally allow verdict constructors.
    Validator,
    /// Procedures additionally allow the stage-2 database builtins.
    Procedure,
    /// Helpers may use every category; the entry ceiling is enforced on the
    /// transitive effects reachable from the entry.
    Helper,
}

impl Capability {
    /// True iff this capability may call builtins of `category`.
    pub fn permits(self, category: BuiltinCategory) -> bool {
        match category {
            BuiltinCategory::Pure => true,
            BuiltinCategory::ValidatorCtor => {
                self == Capability::Validator || self == Capability::Helper
            }
            BuiltinCategory::Stage2 => self == Capability::Procedure || self == Capability::Helper,
        }
    }
}

/// Observed effects of one function body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EffectSet {
    validator_ctors: bool,
    stage2: bool,
}

impl EffectSet {
    /// Creates an empty effect set.
    pub fn new() -> Self {
        Self {
            validator_ctors: false,
            stage2: false,
        }
    }

    /// Records one builtin call category.
    pub fn record(&mut self, category: BuiltinCategory) {
        match category {
            BuiltinCategory::ValidatorCtor => self.validator_ctors = true,
            BuiltinCategory::Stage2 => self.stage2 = true,
            BuiltinCategory::Pure => {}
        }
    }

    /// True iff any validator constructor was observed.
    pub fn validator_ctors(self) -> bool {
        self.validator_ctors
    }

    /// True iff any stage-2 database builtin was observed.
    pub fn stage2(self) -> bool {
        self.stage2
    }

    /// Merges `other` into this set.
    pub fn union(&mut self, other: EffectSet) {
        self.validator_ctors = self.validator_ctors || other.validator_ctors;
        self.stage2 = self.stage2 || other.stage2;
    }
}
