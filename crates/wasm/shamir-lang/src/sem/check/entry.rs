use std::collections::BTreeMap;

use crate::api::{CompileLimits, Diagnostic, Span, Stage};
use crate::front::budget::CompileBudget;
use crate::front::{
    self, EntryKind, ExprId, FunctionId, PrimitiveType, TypeFieldId, TypeId as AstTypeId, TypeKind,
};
use crate::sem::effects::{builtin_by_name, Capability, EffectSet};
use crate::sem::tables::{CheckedCall, TypedModule};
use crate::sem::types::{TypeData, TypeId, TypeInterner};

use super::{calls, resolve, statement};

/// Maximum number of retained diagnostics.
pub(super) const MAX_DIAGNOSTICS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlanOperation {
    Read,
    Insert,
    Update,
    Delete,
}

#[derive(Clone, Debug)]
pub(super) enum PlanFact {
    Batch {
        operations: Vec<(String, PlanOperation)>,
        unknown_alias: bool,
    },
    Request {
        operations: Vec<(String, PlanOperation)>,
        unknown_alias: bool,
    },
    Response {
        operations: Vec<(String, PlanOperation)>,
        unknown_alias: bool,
    },
    Unknown,
}

/// Shared semantic-checker context; arena-parallel tables indexed by id.
pub(super) struct Checker<'a> {
    pub(super) module: &'a front::Module,
    pub(super) limits: &'a CompileLimits,
    pub(super) stage: Stage,
    pub(super) callable_catalog: &'a std::collections::BTreeMap<String, Vec<u8>>,
    pub(super) types: TypeInterner,
    pub(super) diags: Vec<Diagnostic>,
    pub(super) type_map: Vec<Option<TypeId>>,
    pub(super) resolved: Vec<Option<crate::sem::ResolvedTarget>>,
    pub(super) calls: Vec<Option<crate::sem::ResolvedCall>>,
    pub(super) plan_facts: Vec<Option<PlanFact>>,
    pub(super) expr_types: Vec<Option<TypeId>>,
    pub(super) binding_slots: Vec<Option<u32>>,
    pub(super) slot_mutability: Vec<Vec<bool>>,
    pub(super) signatures: Vec<ResolvedSignature>,
    pub(super) entry_kinds: Vec<Option<EntryKind>>,
    pub(super) own_effects: Vec<EffectSet>,
    pub(super) by_name: BTreeMap<String, FunctionId>,
    pub(super) edges: Vec<CallEdge>,
    pub(super) checked_calls: Vec<Option<CheckedCall>>,
    pub(super) budget: &'a mut CompileBudget,
    /// Canonical ids for primitive types (`TypeInterner::intern` does not
    /// deduplicate, so equality-based assignability needs one id per name).
    pub(super) primitives: BTreeMap<&'static str, TypeId>,
    /// Canonical ids for optional/list constructors, keyed structurally.
    pub(super) structural: BTreeMap<String, TypeId>,
}

/// One user-function call edge recorded during resolution.
pub(super) struct CallEdge {
    pub(super) caller: FunctionId,
    pub(super) callee: FunctionId,
    pub(super) site: ExprId,
}

/// Resolved function signature.
pub(super) struct ResolvedSignature {
    pub(super) params: Vec<TypeId>,
    pub(super) result: TypeId,
}

/// Position of a type conversion; drives null/opaque acceptance rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TopPosition {
    Return,
    Parameter,
    Local,
    Nested,
    Field,
}

/// Runs the full milestone-3 semantic check over one parsed module.
pub fn check(
    module: &front::Module,
    limits: &CompileLimits,
    stage: Stage,
) -> Result<TypedModule, Vec<Diagnostic>> {
    let normalized = match limits.clamped() {
        Ok(normalized) => normalized,
        Err(error) => {
            return Err(vec![Diagnostic::new(
                "B001",
                format!("invalid zero compiler limit: {}", error.field),
                module.span,
            )]);
        }
    };
    let mut budget = match CompileBudget::new(&normalized) {
        Ok(budget) => budget,
        Err(_) => {
            return Err(vec![Diagnostic::new(
                "B001",
                "invalid zero compiler limit".to_string(),
                module.span,
            )]);
        }
    };
    check_scoped_with_catalog(
        module,
        &normalized,
        stage,
        &std::collections::BTreeMap::new(),
        &mut budget,
    )
}

pub fn check_scoped(
    module: &front::Module,
    limits: &CompileLimits,
    stage: Stage,
    budget: &mut CompileBudget,
) -> Result<TypedModule, Vec<Diagnostic>> {
    check_scoped_with_catalog(
        module,
        limits,
        stage,
        &std::collections::BTreeMap::new(),
        budget,
    )
}

/// Runs the full semantic checker with a caller-supplied callable catalog.
pub(crate) fn check_scoped_with_catalog(
    module: &front::Module,
    limits: &CompileLimits,
    stage: Stage,
    callable_catalog: &std::collections::BTreeMap<String, Vec<u8>>,
    budget: &mut CompileBudget,
) -> Result<TypedModule, Vec<Diagnostic>> {
    let normalized = limits;
    let node_count = u64::try_from(module.node_count()).unwrap_or(u64::MAX);
    if let Err(diag) = budget.charge(node_count, module.span) {
        return Err(vec![diag]);
    }
    let mut checker = Checker {
        module,
        limits: normalized,
        stage,
        callable_catalog,
        types: TypeInterner::new(),
        diags: Vec::new(),
        type_map: vec![None; module.types.len()],
        resolved: vec![None; module.expressions.len()],
        calls: vec![None; module.expressions.len()],
        plan_facts: vec![None; module.expressions.len()],
        expr_types: vec![None; module.expressions.len()],
        binding_slots: vec![None; module.statements.len()],
        slot_mutability: (0..module.functions.len()).map(|_| Vec::new()).collect(),
        signatures: Vec::new(),
        entry_kinds: Vec::new(),
        own_effects: Vec::new(),
        by_name: BTreeMap::new(),
        edges: Vec::new(),
        checked_calls: vec![None; module.expressions.len()],
        budget,
        primitives: BTreeMap::new(),
        structural: BTreeMap::new(),
    };
    if module.functions.len() > normalized.max_functions {
        report(
            &mut checker,
            "B001",
            "too many functions".to_string(),
            module.span,
        );
    }
    register_functions(&mut checker);
    resolve::resolve_module(&mut checker);
    let call_order = calls::build_call_order(&mut checker);
    let mut functions = statement::check_all_bodies(&mut checker);
    let transitive = calls::check_entry_capabilities(&mut checker, &call_order);
    for typed in functions.iter_mut() {
        if let Some(effects) = transitive.get(typed.id.0 as usize) {
            typed.effects = *effects;
        }
    }
    let entry_index = checker.entry_kinds.iter().position(|kind| kind.is_some());
    let Some(entry_index) = entry_index else {
        report(
            &mut checker,
            "T009",
            "module requires exactly one @entry declaration".to_string(),
            module.span,
        );
        return Err(finish(checker.diags));
    };
    let entry = FunctionId(entry_index as u32);
    let entry_kind = checker
        .entry_kinds
        .get(entry_index)
        .copied()
        .flatten()
        .unwrap_or(EntryKind::Scalar);
    let span = module.span;
    let error_id = checker.types.error();
    let leaked = checker
        .expr_types
        .iter()
        .flatten()
        .any(|ty| *ty == error_id)
        || functions.iter().any(|func| {
            func.return_type == error_id
                || func.parameters.iter().any(|param| param.ty == error_id)
                || func.locals.iter().any(|local| local.ty == error_id)
        });
    let Checker {
        types,
        diags,
        resolved,
        calls,
        checked_calls,
        expr_types,
        binding_slots,
        ..
    } = checker;
    let typed = TypedModule {
        types,
        functions,
        call_order,
        entry,
        entry_kind,
        expr_types,
        resolved,
        calls,
        checked_calls,
        binding_slots,
        span,
    };
    let mut diags = finish(diags);
    if diags.is_empty() && leaked {
        diags.push(Diagnostic::new(
            "I001",
            "internal invariant: error type leaked into a clean module",
            module.span,
        ));
    }
    if diags.is_empty() {
        Ok(typed)
    } else {
        Err(diags)
    }
}

/// Sorts, dedupes and caps the diagnostic list deterministically.
fn finish(mut diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diags.sort_by(|a, b| (a.primary.start_byte, a.code).cmp(&(b.primary.start_byte, b.code)));
    diags.dedup_by(|a, b| {
        a.primary.start_byte == b.primary.start_byte
            && a.primary.end_byte == b.primary.end_byte
            && a.code == b.code
    });
    diags.truncate(MAX_DIAGNOSTICS);
    diags
}

/// Pass 1: registers names, resolves signatures and checks entry boundaries.
fn register_functions(c: &mut Checker) {
    let count = c.module.functions.len();
    for index in 0..count {
        let fid = FunctionId(index as u32);
        let func = match c.module.function(fid) {
            Some(func) => func,
            None => continue,
        };
        let span = func.span;
        let name = func.name.clone();
        let kind = func.entry_kind;
        let in_entry = kind.is_some();
        if builtin_by_name(&name).is_some() {
            report(c, "N002", format!("function name {name} is reserved"), span);
        } else if c.by_name.contains_key(&name) {
            report(c, "N002", format!("duplicate function {name}"), span);
        } else {
            c.by_name.insert(name.clone(), fid);
        }
        check_parameters(c, fid, span);
        let params_typed = convert_parameters(c, fid, in_entry, kind);
        let return_span = c
            .module
            .ty(func.return_type)
            .map(|ty| ty.span)
            .unwrap_or(span);
        let result = convert_type(c, func.return_type, TopPosition::Return, in_entry, kind, 0);
        c.signatures.push(ResolvedSignature {
            params: params_typed.iter().map(|(ty, _)| *ty).collect(),
            result,
        });
        c.entry_kinds.push(kind);
        c.own_effects.push(EffectSet::new());
        if let Some(kind) = kind {
            check_entry_boundary(c, fid, kind, &params_typed, result, return_span);
        }
    }
}

/// True iff `name` is a canonical decimal string: non-empty, all ASCII
/// digits, and no leading zero unless the name is exactly "0".
fn is_canonical_decimal(name: &str) -> bool {
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    name == "0" || !name.starts_with('0')
}

/// Pass-1 parameter checks: arity cap, duplicate names, reserved names.
fn check_parameters(c: &mut Checker, fid: FunctionId, span: Span) {
    let Some(func) = c.module.function(fid) else {
        return;
    };
    if func.parameters.len() > c.limits.max_parameters {
        report(c, "B001", "too many parameters".to_string(), span);
    }
    let mut seen: Vec<String> = Vec::new();
    for pid in &func.parameters {
        let Some(param) = c.module.parameter(*pid) else {
            continue;
        };
        let name = param.name.clone();
        if seen.contains(&name) {
            report(c, "N002", format!("duplicate parameter {name}"), param.span);
            continue;
        }
        if name == "args" {
            report(
                c,
                "N002",
                "<args> is reserved as a parameter name".to_string(),
                param.span,
            );
        } else if builtin_by_name(&name).is_some() {
            report(
                c,
                "N002",
                format!("parameter name {name} is reserved"),
                param.span,
            );
        } else {
            // a canonical decimal parameter name would be ambiguous against
            // the positional envelope keys (design section 3), so entries
            // reject it at compile time
            if func.entry_kind.is_some() && is_canonical_decimal(&name) {
                report(
                    c,
                    "T009",
                    format!("parameter name {name} collides with the positional envelope key"),
                    param.span,
                );
            }
            seen.push(name);
        }
    }
}

/// Converts every parameter type; returns (type, type-node span) pairs.
fn convert_parameters(
    c: &mut Checker,
    fid: FunctionId,
    in_entry: bool,
    kind: Option<EntryKind>,
) -> Vec<(TypeId, Span)> {
    let mut typed: Vec<(TypeId, Span)> = Vec::new();
    let Some(func) = c.module.function(fid) else {
        return typed;
    };
    for pid in &func.parameters {
        let Some(param) = c.module.parameter(*pid) else {
            continue;
        };
        let ty_span = c
            .module
            .ty(param.ty)
            .map(|ty| ty.span)
            .unwrap_or(param.span);
        let ty = convert_type(c, param.ty, TopPosition::Parameter, in_entry, kind, 0);
        typed.push((ty, ty_span));
    }
    typed
}

/// Entry-boundary rules: wire safety, stage gating and validator shape.
fn check_entry_boundary(
    c: &mut Checker,
    fid: FunctionId,
    kind: EntryKind,
    params: &[(TypeId, Span)],
    result: TypeId,
    result_span: Span,
) {
    let Some(func) = c.module.function(fid) else {
        return;
    };
    match kind {
        EntryKind::Scalar | EntryKind::Procedure => {
            if kind == EntryKind::Procedure && c.stage == Stage::Mvp {
                report(
                    c,
                    "T007",
                    "procedure entries are not available in the MVP stage".to_string(),
                    func.span,
                );
            }
            for (ty, span) in params {
                if !c.types.wire_safe(*ty, 0) && !c.types.contains_error(*ty, 0) {
                    report(
                        c,
                        "T009",
                        "entry parameter/result is not a wire-safe type".to_string(),
                        *span,
                    );
                }
            }
            let null_result =
                kind == EntryKind::Procedure && c.types.data(result) == Some(TypeData::Null);
            if !c.types.wire_safe(result, 0) && !c.types.contains_error(result, 0) && !null_result {
                report(
                    c,
                    "T009",
                    "entry parameter/result is not a wire-safe type".to_string(),
                    result_span,
                );
            }
        }
        EntryKind::Validator => {
            if func.parameters.len() != 2 {
                report(
                    c,
                    "T009",
                    "validator requires exactly two parameters".to_string(),
                    func.span,
                );
            }
            let expected_names = ["record", "old_record"];
            for (position, pid) in func.parameters.iter().enumerate() {
                let Some(param) = c.module.parameter(*pid) else {
                    continue;
                };
                let name_ok = expected_names
                    .get(position)
                    .map(|expected| *expected == param.name)
                    .unwrap_or(false);
                if !name_ok {
                    report(
                        c,
                        "T009",
                        "validator parameters must be named record and old_record".to_string(),
                        param.span,
                    );
                }
                if !is_optional_record(&c.types, params.get(position).map(|(ty, _)| *ty)) {
                    report(
                        c,
                        "T009",
                        "validator parameters must be optional records".to_string(),
                        params
                            .get(position)
                            .map(|(_, span)| *span)
                            .unwrap_or(param.span),
                    );
                }
            }
            if c.types.data(result) != Some(TypeData::Validation) {
                report(
                    c,
                    "T009",
                    "validator must return Validation".to_string(),
                    result_span,
                );
            }
        }
    }
}

/// True iff `id` is `Optional(record)`; `None`/other shapes report false.
fn is_optional_record(types: &TypeInterner, id: Option<TypeId>) -> bool {
    let Some(id) = id else {
        return false;
    };
    match types.data(id) {
        Some(TypeData::Optional(inner)) => {
            matches!(types.data(inner), Some(TypeData::Record(_)))
        }
        _ => false,
    }
}

/// Reports one diagnostic unless the cap is reached.
pub(super) fn report(c: &mut Checker, code: &'static str, message: String, span: Span) {
    if c.diags.len() >= MAX_DIAGNOSTICS {
        return;
    }
    c.diags.push(Diagnostic::new(code, message, span));
}

/// Reports one diagnostic with a related location unless the cap is reached.
pub(super) fn report_related(
    c: &mut Checker,
    code: &'static str,
    message: String,
    primary: Span,
    related_span: Span,
    related_message: &str,
) {
    if c.diags.len() >= MAX_DIAGNOSTICS {
        return;
    }
    let diag = Diagnostic::new(code, message, primary).with_related(related_span, related_message);
    c.diags.push(diag);
}

/// The interner's suppression type, interned exactly once.
pub(super) fn error_type(c: &mut Checker) -> TypeId {
    c.types.error()
}

/// Canonical primitive type id for one key (`TypeInterner` does not dedupe).
pub(super) fn primitive_type(c: &mut Checker, key: &'static str) -> TypeId {
    if let Some(&id) = c.primitives.get(key) {
        return id;
    }
    let data = match key {
        "i32" => TypeData::I32,
        "i64" => TypeData::I64,
        "f64" => TypeData::F64,
        "bool" => TypeData::Bool,
        "string" => TypeData::String,
        "bytes" => TypeData::Bytes,
        "null" => TypeData::Null,
        "validation" => TypeData::Validation,
        "batch" => TypeData::Batch,
        "request" => TypeData::Request,
        "response" => TypeData::Response,
        _ => TypeData::Error,
    };
    let id = c.types.intern(data);
    c.primitives.insert(key, id);
    id
}

/// Canonical `Optional(inner)` id.
pub(super) fn intern_optional(c: &mut Checker, inner: TypeId) -> TypeId {
    let key = format!("o{}", inner.0);
    if let Some(&id) = c.structural.get(&key) {
        return id;
    }
    let id = c.types.intern(TypeData::Optional(inner));
    c.structural.insert(key, id);
    id
}

/// Canonical `List(inner)` id.
pub(super) fn intern_list(c: &mut Checker, inner: TypeId) -> TypeId {
    let key = format!("l{}", inner.0);
    if let Some(&id) = c.structural.get(&key) {
        return id;
    }
    let id = c.types.intern(TypeData::List(inner));
    c.structural.insert(key, id);
    id
}

/// Maps an AST type id to its interned semantic type.
pub(super) fn type_of_ast(c: &mut Checker, ast: AstTypeId) -> Option<TypeId> {
    c.type_map.get(ast.0 as usize).copied().flatten()
}

/// Fallback span for an expression lookup failure.
pub(super) fn expr_span(c: &Checker, id: ExprId) -> Span {
    c.module
        .expression(id)
        .map(|expr| expr.span)
        .unwrap_or(c.module.span)
}

/// Capability ceiling for one entry kind; helpers may use every category and
/// are restricted by the transitive effect walk from their entry.
pub(super) fn capability_of(kind: Option<EntryKind>) -> Capability {
    match kind {
        Some(EntryKind::Scalar) => Capability::Scalar,
        Some(EntryKind::Validator) => Capability::Validator,
        Some(EntryKind::Procedure) => Capability::Procedure,
        None => Capability::Helper,
    }
}

/// Converts one AST type into an interned semantic type (cached per node).
pub(super) fn convert_type(
    c: &mut Checker,
    ast: AstTypeId,
    top: TopPosition,
    in_entry: bool,
    kind: Option<EntryKind>,
    depth: u32,
) -> TypeId {
    let span = match c.module.ty(ast) {
        Some(ty) => ty.span,
        None => return error_type(c),
    };
    if let Some(cached) = type_of_ast(c, ast) {
        return cached;
    }
    if depth > c.limits.max_type_depth as u32 {
        report(
            c,
            "B001",
            "type nesting exceeds the limit".to_string(),
            span,
        );
        return error_type(c);
    }
    let node_kind = match c.module.ty(ast) {
        Some(ty) => ty.kind.clone(),
        None => return error_type(c),
    };
    let result = match node_kind {
        TypeKind::Primitive(prim) => convert_primitive(c, top, in_entry, kind, prim, span),
        TypeKind::Optional(child) => {
            let inner = convert_type(c, child, TopPosition::Nested, in_entry, kind, depth + 1);
            intern_optional(c, inner)
        }
        TypeKind::List(child) => {
            let inner = convert_type(c, child, TopPosition::Nested, in_entry, kind, depth + 1);
            intern_list(c, inner)
        }
        TypeKind::Record(field_ids) => convert_record(c, &field_ids, in_entry, kind, depth, span),
    };
    if let Some(slot) = c.type_map.get_mut(ast.0 as usize) {
        *slot = Some(result);
    }
    result
}

/// Converts one primitive type per the null/opaque position rules.
fn convert_primitive(
    c: &mut Checker,
    top: TopPosition,
    in_entry: bool,
    kind: Option<EntryKind>,
    prim: PrimitiveType,
    span: Span,
) -> TypeId {
    match prim {
        PrimitiveType::Null => {
            let allowed = match top {
                TopPosition::Return => {
                    !in_entry
                        || kind == Some(EntryKind::Validator)
                        || kind == Some(EntryKind::Procedure)
                }
                TopPosition::Parameter => !in_entry,
                TopPosition::Local => !in_entry || kind == Some(EntryKind::Validator),
                TopPosition::Nested | TopPosition::Field => {
                    report(c, "T009", "null is not permitted here".to_string(), span);
                    return error_type(c);
                }
            };
            if !allowed {
                report(
                    c,
                    "T009",
                    "null type is only permitted as a return type".to_string(),
                    span,
                );
                return error_type(c);
            }
            primitive_type(c, "null")
        }
        PrimitiveType::Validation => {
            if matches!(top, TopPosition::Nested | TopPosition::Field) {
                report(
                    c,
                    "T009",
                    "opaque type is not permitted here".to_string(),
                    span,
                );
                return error_type(c);
            }
            let allowed = match top {
                TopPosition::Return | TopPosition::Local => {
                    !in_entry || kind == Some(EntryKind::Validator)
                }
                TopPosition::Parameter => !in_entry,
                TopPosition::Nested | TopPosition::Field => false,
            };
            if !allowed {
                report(
                    c,
                    "T009",
                    "Validation is only permitted for validator results and internal helpers"
                        .to_string(),
                    span,
                );
                return error_type(c);
            }
            primitive_type(c, "validation")
        }
        PrimitiveType::Batch | PrimitiveType::Request | PrimitiveType::Response => {
            if matches!(top, TopPosition::Nested | TopPosition::Field) {
                report(
                    c,
                    "T009",
                    "opaque type is not permitted here".to_string(),
                    span,
                );
                return error_type(c);
            }
            if in_entry && matches!(top, TopPosition::Return | TopPosition::Parameter) {
                report(
                    c,
                    "T009",
                    "opaque stage-2 types are not permitted at entry boundaries".to_string(),
                    span,
                );
                return error_type(c);
            }
            primitive_type(
                c,
                match prim {
                    PrimitiveType::Batch => "batch",
                    PrimitiveType::Request => "request",
                    _ => "response",
                },
            )
        }
        PrimitiveType::I32 => primitive_type(c, "i32"),
        PrimitiveType::I64 => primitive_type(c, "i64"),
        PrimitiveType::F64 => primitive_type(c, "f64"),
        PrimitiveType::Bool => primitive_type(c, "bool"),
        PrimitiveType::String => primitive_type(c, "string"),
        PrimitiveType::Bytes => primitive_type(c, "bytes"),
    }
}

/// Converts one record type: duplicate-name rejection and field cap.
fn convert_record(
    c: &mut Checker,
    field_ids: &[TypeFieldId],
    in_entry: bool,
    kind: Option<EntryKind>,
    depth: u32,
    span: Span,
) -> TypeId {
    let cap = c.limits.max_record_fields;
    if field_ids.len() > cap {
        report(c, "B001", "too many record fields".to_string(), span);
    }
    let mut fields: Vec<(String, TypeId)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for field_id in field_ids {
        let Some(field) = c.module.type_field(*field_id) else {
            continue;
        };
        if seen.contains(&field.name) {
            report(
                c,
                "N002",
                format!("duplicate record field {}", field.name),
                field.span,
            );
            continue;
        }
        seen.push(field.name.clone());
        if fields.len() >= cap {
            continue;
        }
        let field_ty = convert_type(c, field.ty, TopPosition::Field, in_entry, kind, depth + 1);
        fields.push((field.name.clone(), field_ty));
    }
    c.types.intern_record(fields)
}
