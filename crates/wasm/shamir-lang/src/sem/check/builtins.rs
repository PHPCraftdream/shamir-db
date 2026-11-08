use crate::api::Span;
use crate::front::ExprId;
use crate::sem::effects::{builtin_category, builtin_name, BuiltinCategory, BuiltinId, Capability};

use crate::sem::types::{TypeData, TypeId};

use super::entry::{error_type, expr_span, intern_list, primitive_type, report, Checker};
use super::expression::{assignable, check_expression, report_mismatch};
use super::statement::FunctionFrame;

/// Checks one builtin call against its signature and the frame capability.
pub(super) fn check_builtin_call(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    bid: BuiltinId,
    call_id: ExprId,
    args: &[ExprId],
    depth: u32,
) -> TypeId {
    let call_span = expr_span(c, call_id);
    let name = builtin_name(bid);
    match builtin_category(bid) {
        BuiltinCategory::Stage2 => {
            report(
                c,
                "T007",
                format!("builtin {name} is not available in this milestone"),
                call_span,
            );
            for &arg in args {
                check_arg(c, frame, None, arg, depth);
            }
            error_type(c)
        }
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
    }
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
