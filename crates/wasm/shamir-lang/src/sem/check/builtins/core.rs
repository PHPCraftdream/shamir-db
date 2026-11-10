use crate::api::Span;
use crate::front::{ExprId, ExprKind};
use crate::sem::effects::{builtin_category, builtin_name, BuiltinCategory, BuiltinId, Capability};

use crate::sem::types::{TypeData, TypeId};

use super::super::entry::{error_type, expr_span, intern_list, primitive_type, report, Checker};
use super::super::expression::{assignable, check_expression, report_mismatch};
use super::super::statement::FunctionFrame;
use super::batch::{
    check_mutation_call, check_response_affected, check_row_projection, record_plan_fact,
};

/// Checks one builtin call against its signature and the frame capability.
pub(in crate::sem::check) fn check_builtin_call(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    bid: BuiltinId,
    call_id: ExprId,
    args: &[ExprId],
    expected: Option<TypeId>,
    depth: u32,
) -> TypeId {
    let call_span = expr_span(c, call_id);
    let result = match builtin_category(bid) {
        BuiltinCategory::Stage2 => match bid {
            BuiltinId::DbGet | BuiltinId::DbInsert | BuiltinId::DbQuery => {
                check_direct_db(c, frame, bid, call_span, args, expected, depth)
            }
            BuiltinId::BatchUpdate | BuiltinId::BatchDelete => {
                check_mutation_call(c, frame, bid, call_span, args, depth)
            }
            BuiltinId::ResponseAffected => {
                check_response_affected(c, frame, call_span, args, expected, depth)
            }
            BuiltinId::CallChecked => super::call_signature::check_checked_call(
                c, frame, call_id, call_span, args, expected, depth,
            ),
            _ => check_batch_family(c, frame, bid, call_span, args, expected, depth),
        },
        BuiltinCategory::ValidatorCtor => {
            let violated = !frame.ceiling.permits(BuiltinCategory::ValidatorCtor);
            check_ctor(c, frame, bid, call_span, args, depth);
            if violated {
                error_type(c)
            } else {
                primitive_type(c, "validation")
            }
        }
        BuiltinCategory::Pure => check_pure(c, frame, bid, call_span, args, depth),
    };
    let plan = if c.types.is_error(result) {
        None
    } else {
        record_plan_fact(c, frame, bid, args)
    };
    if let Some(slot) = c.plan_facts.get_mut(call_id.0 as usize) {
        *slot = plan;
    }
    result
}

/// Checks one argument, delegating to the expression pass.
fn check_arg(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    expected: Option<TypeId>,
    id: ExprId,
    depth: u32,
) -> TypeId {
    check_expression(c, frame, expected, id, depth)
}

/// Reports an arity mismatch; true when the arity matches.
fn check_arity(
    c: &mut Checker,
    name: &str,
    args: &[ExprId],
    expected: usize,
    call_span: Span,
) -> bool {
    if args.len() == expected {
        return true;
    }
    report(
        c,
        "T004",
        format!("builtin {name} expects {expected} arguments"),
        call_span,
    );
    false
}

/// Checks a validator-verdict constructor call.
fn check_ctor(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    bid: BuiltinId,
    call_span: Span,
    args: &[ExprId],
    depth: u32,
) {
    let name = builtin_name(bid);
    let string = primitive_type(c, "string");
    let validation = primitive_type(c, "validation");
    let params: Vec<TypeId> = match bid {
        BuiltinId::Accept => Vec::new(),
        BuiltinId::Reject => vec![string, string],
        BuiltinId::RejectPath => vec![intern_list(c, string), string],
        BuiltinId::RecordError => vec![string],
        BuiltinId::WithError => vec![validation, string, string],
        BuiltinId::WithRecordError => vec![validation, string],
        BuiltinId::Stop => vec![validation],
        // Other builtins never reach this category branch.
        _ => return,
    };
    if args.len() != params.len() {
        report(
            c,
            "T004",
            format!("builtin {name} expects {} arguments", params.len()),
            call_span,
        );
    }
    for (position, &arg) in args.iter().enumerate() {
        match params.get(position).copied() {
            Some(expected) => {
                let actual = check_arg(c, frame, Some(expected), arg, depth);
                if !c.types.is_error(expected)
                    && !c.types.is_error(actual)
                    && !assignable(c, expected, actual)
                {
                    report_mismatch(c, expected, actual, arg, expr_span(c, arg), "argument");
                }
            }
            None => {
                check_arg(c, frame, None, arg, depth);
            }
        }
    }
    if let Some(effects) = c.own_effects.get_mut(frame.fid.0 as usize) {
        effects.record(BuiltinCategory::ValidatorCtor);
    }
    if !frame.ceiling.permits(BuiltinCategory::ValidatorCtor) {
        let ceiling_name = match frame.ceiling {
            Capability::Scalar => "scalar",
            Capability::Procedure => "procedure",
            Capability::Validator => "validator",
            Capability::Helper => "helper",
        };
        report(
            c,
            "T007",
            format!("builtin {name} exceeds the {ceiling_name} entry capability"),
            call_span,
        );
    }
}

/// Checks pure builtin calls with their mono signatures.
fn check_pure(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    bid: BuiltinId,
    call_span: Span,
    args: &[ExprId],
    depth: u32,
) -> TypeId {
    let name = builtin_name(bid);
    let mono = match bid {
        BuiltinId::ToI64 => (("i32", "i64"), 1),
        BuiltinId::ToF64I32 => (("i32", "f64"), 1),
        BuiltinId::ToF64I64 => (("i64", "f64"), 1),
        BuiltinId::ToI32 => (("i64", "i32"), 1),
        BuiltinId::ToI32F64 => (("f64", "i32"), 1),
        BuiltinId::ToI64F64 => (("f64", "i64"), 1),
        // Non-cast builtins are handled below; unreachable here.
        _ => (("", ""), 0),
    };
    if matches!(
        bid,
        BuiltinId::ToI64
            | BuiltinId::ToF64I32
            | BuiltinId::ToF64I64
            | BuiltinId::ToI32
            | BuiltinId::ToI32F64
            | BuiltinId::ToI64F64
    ) {
        let (keys, arity) = mono;
        if !check_arity(c, name, args, arity, call_span) {
            for &arg in args {
                check_arg(c, frame, None, arg, depth);
            }
            return error_type(c);
        }
        let arg = args.first().copied().unwrap_or(ExprId(u32::MAX));
        let expected = primitive_type(c, keys.0);
        let actual = check_arg(c, frame, Some(expected), arg, depth);
        if !assignable(c, expected, actual) {
            report(
                c,
                "T001",
                format!("builtin {name} expects {}", keys.0),
                expr_span(c, arg),
            );
        }
        return primitive_type(c, keys.1);
    }
    match bid {
        BuiltinId::Len => {
            if !check_arity(c, name, args, 1, call_span) {
                return error_type(c);
            }
            let arg = args.first().copied().unwrap_or(ExprId(u32::MAX));
            let actual = check_arg(c, frame, None, arg, depth);
            let accepted = matches!(
                c.types.data(actual),
                Some(TypeData::String)
                    | Some(TypeData::Bytes)
                    | Some(TypeData::List(_))
                    | Some(TypeData::Error)
            );
            if !accepted {
                report(
                    c,
                    "T001",
                    "len expects string, bytes or a list".to_string(),
                    expr_span(c, arg),
                );
            }
            primitive_type(c, "i32")
        }
        BuiltinId::IsNull => {
            if !check_arity(c, name, args, 1, call_span) {
                return error_type(c);
            }
            let arg = args.first().copied().unwrap_or(ExprId(u32::MAX));
            let actual = check_arg(c, frame, None, arg, depth);
            let accepted = matches!(
                c.types.data(actual),
                Some(TypeData::Optional(_)) | Some(TypeData::Null) | Some(TypeData::Error)
            );
            if !accepted {
                report(
                    c,
                    "T001",
                    "is_null expects an optional value".to_string(),
                    expr_span(c, arg),
                );
            }
            primitive_type(c, "bool")
        }
        BuiltinId::Unwrap => {
            if !check_arity(c, name, args, 1, call_span) {
                return error_type(c);
            }
            let arg = args.first().copied().unwrap_or(ExprId(u32::MAX));
            let actual = check_arg(c, frame, None, arg, depth);
            match c.types.data(actual) {
                Some(TypeData::Optional(inner)) => inner,
                Some(TypeData::Null) => {
                    report(
                        c,
                        "T001",
                        "no optional type to unwrap".to_string(),
                        expr_span(c, arg),
                    );
                    error_type(c)
                }
                Some(TypeData::Error) => error_type(c),
                _ => {
                    report(
                        c,
                        "T001",
                        "unwrap expects an optional value".to_string(),
                        expr_span(c, arg),
                    );
                    error_type(c)
                }
            }
        }
        // Validator constructors and stage-2 builtins never reach this branch.
        _ => error_type(c),
    }
}

/// True iff `ty` is one of the closed scalar types (`i32 | i64 | f64 | bool |
/// string | bytes`); `null` is handled by the callers that allow it.
fn is_scalar(c: &Checker, ty: TypeId) -> bool {
    matches!(
        c.types.data(ty),
        Some(
            TypeData::I32
                | TypeData::I64
                | TypeData::F64
                | TypeData::Bool
                | TypeData::String
                | TypeData::Bytes
        )
    )
}

/// True iff `ty` is a structural record whose every field is a scalar,
/// optionally nullable (the design's `ScalarRecord`).
fn is_scalar_record(c: &Checker, ty: TypeId) -> bool {
    let Some(TypeData::Record(index)) = c.types.data(ty) else {
        return false;
    };
    let Some(shape) = c.types.record(index) else {
        return false;
    };
    shape.fields.iter().all(|(_, field)| {
        is_scalar(c, *field)
            || matches!(
                c.types.data(*field),
                Some(TypeData::Optional(inner)) if is_scalar(c, inner)
            )
    })
}

/// Checks `db_get` / `db_insert` / `db_query`: the table is a string, the
/// second argument follows the intrinsic's rule and the result type is the
/// required contextual expected type (`T010` when it is absent or has the
/// wrong shape).
fn check_direct_db(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    bid: BuiltinId,
    call_span: Span,
    args: &[ExprId],
    expected: Option<TypeId>,
    depth: u32,
) -> TypeId {
    let name = builtin_name(bid);
    if !check_arity(c, name, args, 2, call_span) {
        for &arg in args {
            check_arg(c, frame, None, arg, depth);
        }
        return error_type(c);
    }
    let table = args.first().copied().unwrap_or(ExprId(u32::MAX));
    let second = args.get(1).copied().unwrap_or(ExprId(u32::MAX));
    let string = primitive_type(c, "string");
    let table_ty = check_arg(c, frame, Some(string), table, depth);
    if !c.types.is_error(table_ty) && !assignable(c, string, table_ty) {
        report_mismatch(c, string, table_ty, table, expr_span(c, table), "argument");
    }
    let second_ty = check_db_second(c, frame, second, depth);
    let second_span = expr_span(c, second);
    if !c.types.is_error(second_ty) {
        let accepted = match bid {
            // a non-null scalar key or a scalar-valued predicate record
            BuiltinId::DbGet => is_scalar(c, second_ty) || is_scalar_record(c, second_ty),
            // a statically typed wire record (nested records and lists allowed)
            BuiltinId::DbInsert => {
                matches!(c.types.data(second_ty), Some(TypeData::Record(_)))
                    && c.types.wire_safe(second_ty, 0)
            }
            // null, a scalar, a scalar record, or an optional of those
            _ => {
                let base = c.types.underlying_optional(second_ty).unwrap_or(second_ty);
                c.types.data(base) == Some(TypeData::Null)
                    || is_scalar(c, base)
                    || is_scalar_record(c, base)
            }
        };
        if !accepted {
            let rule = match bid {
                BuiltinId::DbGet => "a non-null scalar or a scalar-valued record",
                BuiltinId::DbInsert => "a wire-safe record",
                _ => "null, a scalar or a scalar-valued record",
            };
            report(
                c,
                "T001",
                format!("builtin {name} expects {rule} as its second argument"),
                second_span,
            );
        }
    }
    if let Some(effects) = c.own_effects.get_mut(frame.fid.0 as usize) {
        effects.record(BuiltinCategory::Stage2);
    }
    if !frame.ceiling.permits(BuiltinCategory::Stage2) {
        report(
            c,
            "T007",
            format!("builtin {name} requires a procedure entry"),
            call_span,
        );
        return error_type(c);
    }
    direct_db_result(c, bid, call_span, expected)
}

/// Resolves the contextual result type of a direct DB builtin or reports
/// `T010`.
fn direct_db_result(
    c: &mut Checker,
    bid: BuiltinId,
    call_span: Span,
    expected: Option<TypeId>,
) -> TypeId {
    let name = builtin_name(bid);
    let Some(expected) = expected else {
        report(
            c,
            "T010",
            format!("builtin {name} needs a contextual result type"),
            call_span,
        );
        return error_type(c);
    };
    if c.types.is_error(expected) {
        return expected;
    }
    let wanted = match bid {
        BuiltinId::DbGet => "an optional record",
        BuiltinId::DbInsert => "a record",
        _ => "a list of records",
    };
    let shape_ok = match (bid, c.types.data(expected)) {
        (BuiltinId::DbGet, Some(TypeData::Optional(inner))) => {
            matches!(c.types.data(inner), Some(TypeData::Record(_)))
        }
        (BuiltinId::DbInsert, Some(TypeData::Record(_))) => true,
        (BuiltinId::DbQuery, Some(TypeData::List(inner))) => {
            matches!(c.types.data(inner), Some(TypeData::Record(_)))
        }
        _ => false,
    };
    if !shape_ok || !c.types.wire_safe(expected, 0) {
        report(
            c,
            "T010",
            format!("builtin {name} needs {wanted} as its contextual result type"),
            call_span,
        );
        return error_type(c);
    }
    expected
}

/// Checks the key/document/filter argument of a direct DB builtin. A record
/// literal has no expected type of its own, so its structural type is
/// inferred from its field values first (nested record literals recurse) and
/// the literal is then checked against that type.
fn check_db_second(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    second: ExprId,
    depth: u32,
) -> TypeId {
    let before = c.diags.len();
    let inferred = infer_record_literal(c, frame, second, depth);
    if c.diags.len() != before {
        return error_type(c);
    }
    check_arg(c, frame, inferred, second, depth)
}

/// The structural type of a record literal built from its field values, or
/// `None` when the expression is not a record literal or repeats a field name
/// (the regular literal check reports that).
fn infer_record_literal(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    id: ExprId,
    depth: u32,
) -> Option<TypeId> {
    let fields = match c.module.expression(id).map(|expr| expr.kind.clone()) {
        Some(ExprKind::Record(fields)) => fields,
        _ => return None,
    };
    let mut typed: Vec<(String, TypeId)> = Vec::with_capacity(fields.len());
    for field_id in fields {
        let (name, value) = {
            let field = c.module.value_field(field_id)?;
            (field.name.clone(), field.value)
        };
        if typed.iter().any(|(existing, _)| *existing == name) {
            return None;
        }
        let ty = match infer_record_literal(c, frame, value, depth) {
            Some(nested) => nested,
            None => check_arg(c, frame, None, value, depth),
        };
        typed.push((name, ty));
    }
    Some(c.types.intern_record(typed))
}

/// Checks the typed builder intrinsics (`batch`, `batch_id`, `batch_read`,
/// `batch_insert`, `request`, `db_execute`, `response_rows`,
/// `response_inserted`): fixed opaque signatures, a filter/document argument
/// following the direct-call rules, and a contextual optional list of records
/// for the two response projections.
fn check_batch_family(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    bid: BuiltinId,
    call_span: Span,
    args: &[ExprId],
    expected: Option<TypeId>,
    depth: u32,
) -> TypeId {
    let name = builtin_name(bid);
    let string = primitive_type(c, "string");
    let batch = primitive_type(c, "batch");
    let request = primitive_type(c, "request");
    let response = primitive_type(c, "response");
    // (parameter types, result) with `None` for the free filter/document slot
    let (params, result): (Vec<Option<TypeId>>, Option<TypeId>) = match bid {
        BuiltinId::Batch => (Vec::new(), Some(batch)),
        BuiltinId::BatchId => (vec![Some(batch), Some(string)], Some(batch)),
        BuiltinId::BatchRead | BuiltinId::BatchInsert => (
            vec![Some(batch), Some(string), Some(string), None],
            Some(batch),
        ),
        BuiltinId::Request => (vec![Some(batch)], Some(request)),
        BuiltinId::DbExecute => (vec![Some(request)], Some(response)),
        BuiltinId::ResponseRows | BuiltinId::ResponseInserted => {
            (vec![Some(response), Some(string)], None)
        }
        _ => {
            report(
                c,
                "T007",
                format!("builtin {name} is not available in this milestone"),
                call_span,
            );
            for &arg in args {
                check_arg(c, frame, None, arg, depth);
            }
            return error_type(c);
        }
    };
    if !check_arity(c, name, args, params.len(), call_span) {
        for &arg in args {
            check_arg(c, frame, None, arg, depth);
        }
        return error_type(c);
    }
    for (position, (&arg, param)) in args.iter().zip(params.iter()).enumerate() {
        match param {
            Some(want) => {
                let actual = check_arg(c, frame, Some(*want), arg, depth);
                if !c.types.is_error(actual) && !assignable(c, *want, actual) {
                    report_mismatch(c, *want, actual, arg, expr_span(c, arg), "argument");
                }
            }
            None => {
                let actual = check_db_second(c, frame, arg, depth);
                check_batch_free_argument(c, bid, position, actual, arg);
            }
        }
    }
    if matches!(bid, BuiltinId::ResponseRows | BuiltinId::ResponseInserted) {
        check_row_projection(c, frame, bid, args);
    }
    if let Some(effects) = c.own_effects.get_mut(frame.fid.0 as usize) {
        effects.record(BuiltinCategory::Stage2);
    }
    if !frame.ceiling.permits(BuiltinCategory::Stage2) {
        report(
            c,
            "T007",
            format!("builtin {name} requires a procedure entry"),
            call_span,
        );
        return error_type(c);
    }
    match result {
        Some(ty) => ty,
        None => response_projection_result(c, bid, call_span, expected),
    }
}

/// The filter of `batch_read` or the document of `batch_insert` follows the
/// direct DB rules (null/scalar/scalar record vs. a wire-safe record).
fn check_batch_free_argument(
    c: &mut Checker,
    bid: BuiltinId,
    _position: usize,
    actual: TypeId,
    arg: ExprId,
) {
    if c.types.is_error(actual) {
        return;
    }
    let accepted = if bid == BuiltinId::BatchInsert {
        matches!(c.types.data(actual), Some(TypeData::Record(_))) && c.types.wire_safe(actual, 0)
    } else {
        // a batch filter is static: null, a scalar or a scalar record (an
        // optional filter would need a runtime branch)
        c.types.data(actual) == Some(TypeData::Null)
            || is_scalar(c, actual)
            || is_scalar_record(c, actual)
    };
    if !accepted {
        let rule = if bid == BuiltinId::BatchInsert {
            "a wire-safe record"
        } else {
            "null, a scalar or a scalar-valued record"
        };
        let name = builtin_name(bid);
        report(
            c,
            "T001",
            format!("builtin {name} expects {rule} as its last argument"),
            expr_span(c, arg),
        );
    }
}

/// The contextual result of `response_rows` / `response_inserted`: an
/// optional list of records, or `T010`.
fn response_projection_result(
    c: &mut Checker,
    bid: BuiltinId,
    call_span: Span,
    expected: Option<TypeId>,
) -> TypeId {
    let name = builtin_name(bid);
    let Some(expected) = expected else {
        report(
            c,
            "T010",
            format!("builtin {name} needs a contextual result type"),
            call_span,
        );
        return error_type(c);
    };
    if c.types.is_error(expected) {
        return expected;
    }
    let shape_ok = match c.types.data(expected) {
        Some(TypeData::Optional(inner)) => match c.types.data(inner) {
            Some(TypeData::List(element)) => {
                matches!(c.types.data(element), Some(TypeData::Record(_)))
            }
            _ => false,
        },
        _ => false,
    };
    if !shape_ok || !c.types.wire_safe(expected, 0) {
        report(
            c,
            "T010",
            format!(
                "builtin {name} needs an optional list of records as its contextual result type"
            ),
            call_span,
        );
        return error_type(c);
    }
    expected
}
