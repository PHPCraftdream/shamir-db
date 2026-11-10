use crate::api::Span;
use crate::front::{EntryKind, ExprId, ExprKind};
use crate::sem::effects::BuiltinCategory;
use crate::sem::tables::CheckedCall;
use crate::sem::types::{TypeData, TypeId};

use super::super::entry::{
    error_type, expr_span, intern_list, intern_optional, primitive_type, report, Checker,
};
use super::super::expression::{check_expression, type_name};
use super::super::statement::FunctionFrame;

#[derive(Clone, Debug)]
enum SigType {
    Primitive(&'static str),
    Unsupported,
    Optional(Box<SigType>),
    List(Box<SigType>),
    Record(Vec<(String, SigType)>),
}

#[derive(Clone, Debug)]
struct Param {
    name: String,
    ty: SigType,
}

/// Checks the statically named checked call against its canonical catalog descriptor.
pub(in crate::sem::check) fn check_checked_call(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    call_id: ExprId,
    span: Span,
    args: &[ExprId],
    expected: Option<TypeId>,
    depth: u32,
) -> TypeId {
    let permitted = frame.kind == Some(EntryKind::Procedure) || frame.kind.is_none();
    if let Some(effects) = c.own_effects.get_mut(frame.fid.0 as usize) {
        effects.record(BuiltinCategory::Stage2);
    }
    if !permitted || !frame.ceiling.permits(BuiltinCategory::Stage2) {
        report(
            c,
            "T007",
            "call requires a procedure entry capability".to_string(),
            span,
        );
    }
    if args.len() != 2 {
        report(
            c,
            "T004",
            "call expects a literal target and named argument record".to_string(),
            span,
        );
        for arg in args {
            check_expression(c, frame, None, *arg, depth);
        }
        return error_type(c);
    }
    let target = args[0];
    let name = match c.module.expression(target).map(|expr| &expr.kind) {
        Some(ExprKind::String(value)) => value.clone(),
        _ => {
            report(
                c,
                "T004",
                "call target must be a string literal".to_string(),
                expr_span(c, target),
            );
            check_expression(c, frame, None, args[1], depth);
            return error_type(c);
        }
    };
    let Some(signature_len) = c.callable_catalog.get(&name).map(Vec::len) else {
        report(
            c,
            "T004",
            format!("unknown callable procedure {name:?}"),
            expr_span(c, target),
        );
        check_expression(c, frame, None, args[1], depth);
        return error_type(c);
    };
    let target_span = expr_span(c, target);
    if signature_len > c.limits.max_static_bytes {
        report(
            c,
            "T004",
            "callable signature exceeds the compiler static-byte bound".to_string(),
            expr_span(c, target),
        );
        check_expression(c, frame, None, args[1], depth);
        return error_type(c);
    }
    let budget_charge = u64::try_from(signature_len).unwrap_or(u64::MAX);
    if c.budget.charge(budget_charge, target_span).is_err() {
        report(
            c,
            "B001",
            "call signature parsing exceeds compile budget".to_string(),
            target_span,
        );
        check_expression(c, frame, None, args[1], depth);
        return error_type(c);
    }
    let Some(signature_bytes) = c.callable_catalog.get(&name).cloned() else {
        report(
            c,
            "T004",
            format!("unknown callable procedure {name:?}"),
            expr_span(c, target),
        );
        check_expression(c, frame, None, args[1], depth);
        return error_type(c);
    };
    let parsed = match parse_signature(
        &signature_bytes,
        c.limits.max_type_depth as u32,
        c.limits.max_parameters,
        c.limits.max_record_fields,
    ) {
        Some(parsed) => parsed,
        None => {
            report(
                c,
                "T004",
                "callable catalog contains an invalid signature descriptor".to_string(),
                span,
            );
            check_expression(c, frame, None, args[1], depth);
            return error_type(c);
        }
    };
    let mut param_types = Vec::with_capacity(parsed.params.len());
    for param in &parsed.params {
        let Some(param_ty) = materialize(c, &param.ty, 0) else {
            report(
                c,
                "T009",
                "call signature contains an unsupported boundary type".to_string(),
                span,
            );
            check_expression(c, frame, None, args[1], depth);
            return error_type(c);
        };
        if !c.types.wire_safe(param_ty, 0) {
            report(
                c,
                "T009",
                "call signature contains a non-wire-safe parameter boundary".to_string(),
                span,
            );
        }
        param_types.push((param.name.clone(), param_ty));
    }
    let record_ty = c.types.intern_record(param_types.clone());
    let actual_record = check_expression(c, frame, Some(record_ty), args[1], depth);
    let mut record_expr = args[1];
    for _ in 0..c.limits.max_ast_depth {
        match c.module.expression(record_expr).map(|expr| &expr.kind) {
            Some(ExprKind::Group(inner)) => record_expr = *inner,
            _ => break,
        }
    }
    let fields = match c.module.expression(record_expr).map(|expr| &expr.kind) {
        Some(ExprKind::Record(fields)) => Some(fields.clone()),
        _ => None,
    };
    if let Some(fields) = fields.as_ref() {
        let mut supplied: Vec<String> = Vec::with_capacity(fields.len());
        for field_id in fields {
            if let Some(field) = c.module.value_field(*field_id) {
                if supplied.iter().any(|name| name == &field.name) {
                    report(
                        c,
                        "T004",
                        format!("duplicate call argument {}", field.name),
                        field.span,
                    );
                }
                supplied.push(field.name.clone());
            }
        }
        if parsed.params.len() != supplied.len()
            || parsed
                .params
                .iter()
                .any(|param| !supplied.contains(&param.name))
            || supplied
                .iter()
                .any(|name| !parsed.params.iter().any(|param| param.name == *name))
        {
            report(
                c,
                "T004",
                "call arguments must name exactly the declared parameters".to_string(),
                expr_span(c, args[1]),
            );
        }
        if !is_exact_call_value(c, args[1], record_ty, depth) {
            report(
                c,
                "T004",
                "call arguments must exactly match the declared parameter types".to_string(),
                expr_span(c, args[1]),
            );
        }
    } else if !c.types.is_error(actual_record) && actual_record != record_ty {
        report(
            c,
            "T004",
            format!(
                "call arguments expect {}, found {}",
                type_name(c, record_ty),
                type_name(c, actual_record)
            ),
            expr_span(c, args[1]),
        );
    }
    if let Some(slot) = c.expr_types.get_mut(args[1].0 as usize) {
        *slot = Some(record_ty);
    }
    let Some(result) = materialize(c, &parsed.result, 0) else {
        report(
            c,
            "T009",
            "call signature contains an unsupported result boundary".to_string(),
            span,
        );
        return error_type(c);
    };

    if c.types.data(result) != Some(TypeData::Null) && !c.types.wire_safe(result, 0) {
        report(
            c,
            "T009",
            "call signature contains a non-wire-safe result boundary".to_string(),
            span,
        );
    }
    if expected.is_some_and(|context| context != result && !c.types.is_error(context)) {
        let context = expected.unwrap_or(result);
        report(
            c,
            "T004",
            format!(
                "call result is {}, not {}",
                type_name(c, result),
                type_name(c, context)
            ),
            span,
        );
    }
    if let Some(slot) = c.checked_calls.get_mut(call_id.0 as usize) {
        *slot = Some(CheckedCall {
            name,
            signature: signature_bytes,
        });
    }
    result
}

struct Signature {
    params: Vec<Param>,
    result: SigType,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    max_depth: u32,
    max_fields: usize,
}

fn parse_signature(
    bytes: &[u8],
    max_depth: u32,
    max_params: usize,
    max_fields: usize,
) -> Option<Signature> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut cursor = Cursor {
        bytes: text.as_bytes(),
        pos: 0,
        max_depth,
        max_fields,
    };
    cursor.expect(b"procedure(")?;
    let mut params = Vec::new();
    if cursor.peek() != Some(b')') {
        loop {
            if params.len() >= max_params {
                return None;
            }
            let name = cursor.identifier()?;
            if params.iter().any(|existing: &Param| existing.name == name) {
                return None;
            }
            cursor.expect(b":")?;
            let ty = cursor.ty(0)?;
            params.push(Param { name, ty });
            match cursor.peek()? {
                b',' => cursor.pos += 1,
                b')' => break,
                _ => return None,
            }
        }
    }
    cursor.expect(b")->")?;
    let result = cursor.ty(0)?;
    (cursor.pos == cursor.bytes.len()).then_some(Signature { params, result })
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn expect(&mut self, token: &[u8]) -> Option<()> {
        let end = self.pos.checked_add(token.len())?;
        if self.bytes.get(self.pos..end)? != token {
            return None;
        }
        self.pos = end;
        Some(())
    }
    fn identifier(&mut self) -> Option<String> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            self.pos += 1;
        }
        if self.pos == start
            || !self.bytes[start].is_ascii_alphabetic() && self.bytes[start] != b'_'
        {
            return None;
        }
        Some(
            std::str::from_utf8(self.bytes.get(start..self.pos)?)
                .ok()?
                .to_string(),
        )
    }
    fn ty(&mut self, depth: u32) -> Option<SigType> {
        if depth > self.max_depth {
            return None;
        }
        let mut ty = match self.peek()? {
            b'[' => {
                self.pos += 1;
                let inner = self.ty(depth + 1)?;
                self.expect(b"]")?;
                SigType::List(Box::new(inner))
            }
            b'{' => {
                self.pos += 1;
                let mut fields = Vec::new();
                if self.peek() != Some(b'}') {
                    loop {
                        if fields.len() >= self.max_fields {
                            return None;
                        }
                        let name = self.identifier()?;
                        self.expect(b":")?;
                        let child = self.ty(depth + 1)?;
                        if fields
                            .last()
                            .is_some_and(|(previous, _): &(String, SigType)| {
                                previous.as_bytes() >= name.as_bytes()
                            })
                        {
                            return None;
                        }
                        fields.push((name, child));
                        match self.peek()? {
                            b',' => self.pos += 1,
                            b'}' => break,
                            _ => return None,
                        }
                    }
                }
                self.expect(b"}")?;
                SigType::Record(fields)
            }
            _ => {
                let name = self.identifier()?;
                SigType::Primitive(match name.as_str() {
                    "i32" => "i32",
                    "i64" => "i64",
                    "f64" => "f64",
                    "bool" => "bool",
                    "string" => "string",
                    "bytes" => "bytes",
                    "null" => "null",
                    "Validation" | "Batch" | "Request" | "Response" => {
                        return Some(SigType::Unsupported)
                    }
                    _ => return None,
                })
            }
        };
        if self.peek() == Some(b'?') {
            self.pos += 1;
            ty = SigType::Optional(Box::new(ty));
        }
        Some(ty)
    }
}

fn is_exact_call_value(c: &Checker, id: ExprId, expected: TypeId, depth: u32) -> bool {
    if depth > c.limits.max_ast_depth as u32 {
        return false;
    }
    let Some(expr) = c.module.expression(id) else {
        return false;
    };
    if let ExprKind::Group(inner) = &expr.kind {
        return is_exact_call_value(c, *inner, expected, depth.saturating_add(1));
    }
    let actual = c.expr_types.get(id.0 as usize).copied().flatten();
    if actual.is_some_and(|actual| c.types.is_error(actual)) {
        return true;
    }
    if matches!(c.types.data(expected), Some(TypeData::Optional(_)))
        && matches!(&expr.kind, ExprKind::Record(_) | ExprKind::List(_))
    {
        return false;
    }
    let exact = actual.is_some_and(|actual| {
        actual == expected
            || matches!(c.types.data(expected), Some(TypeData::Optional(_)))
                && c.types.data(actual) == Some(TypeData::Null)
    });
    if !exact {
        return false;
    }
    let expected = match c.types.data(expected) {
        Some(TypeData::Optional(inner))
            if actual == Some(expected)
                || actual.is_some_and(|actual| c.types.data(actual) == Some(TypeData::Null)) =>
        {
            inner
        }
        _ => expected,
    };
    match (&expr.kind, c.types.data(expected)) {
        (ExprKind::Record(fields), Some(TypeData::Record(index))) => {
            let Some(shape) = c.types.record(index) else {
                return false;
            };
            if fields.len() != shape.fields.len() {
                return false;
            }
            fields.iter().all(|field_id| {
                let Some(field) = c.module.value_field(*field_id) else {
                    return false;
                };
                shape.field(&field.name).is_some_and(|field_ty| {
                    is_exact_call_value(c, field.value, field_ty, depth.saturating_add(1))
                })
            })
        }
        (ExprKind::List(items), Some(TypeData::List(elem))) => items
            .iter()
            .all(|item| is_exact_call_value(c, *item, elem, depth.saturating_add(1))),
        _ => true,
    }
}

fn materialize(c: &mut Checker, ty: &SigType, depth: u32) -> Option<TypeId> {
    if depth > c.limits.max_type_depth as u32 {
        return None;
    }
    match ty {
        SigType::Primitive(name) => Some(primitive_type(c, name)),
        SigType::Unsupported => None,
        SigType::Optional(inner) => {
            let child = materialize(c, inner, depth + 1)?;
            Some(intern_optional(c, child))
        }
        SigType::List(inner) => {
            let child = materialize(c, inner, depth + 1)?;
            Some(intern_list(c, child))
        }
        SigType::Record(fields) => {
            let mut shape = Vec::with_capacity(fields.len());
            for (name, field) in fields {
                let ty = materialize(c, field, depth + 1)?;
                shape.push((name.clone(), ty));
            }
            Some(c.types.intern_record(shape))
        }
    }
}
