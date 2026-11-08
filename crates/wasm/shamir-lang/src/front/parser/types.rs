use crate::api::Span;
use crate::front::{Keyword, Punctuation, TokenKind};

use super::super::ast::{PrimitiveType, Type, TypeField, TypeId, TypeKind};
use super::parse::{join_span, Parser};

/// Parses one recursive type production within both independent depth limits.
pub(super) fn parse_type(
    parser: &mut Parser<'_>,
    depth: usize,
) -> Result<TypeId, crate::api::Diagnostic> {
    if depth > parser.limits.max_type_depth {
        return Err(parser.bound_error(
            "type depth",
            parser.limits.max_type_depth,
            parser.current_span(),
        ));
    }
    parser.reserve_type(parser.current_span())?;
    let opening = parser.current_span();
    parser.enter(opening)?;
    let result = parse_type_inner(parser, depth, opening);
    parser.leave();
    result
}

fn parse_type_inner(
    parser: &mut Parser<'_>,
    depth: usize,
    start: Span,
) -> Result<TypeId, crate::api::Diagnostic> {
    let base_kind = match parser.current().map(|token| &token.kind) {
        Some(TokenKind::Keyword(keyword)) => {
            let Some(primitive) = primitive(*keyword) else {
                return Err(parser.expected("a type"));
            };
            parser.advance()?;
            TypeKind::Primitive(primitive)
        }
        Some(TokenKind::Punctuation(Punctuation::LeftBracket)) => {
            parser.advance()?;
            let child = parse_type(parser, depth.saturating_add(1))?;
            parser.expect_punctuation(Punctuation::RightBracket, "']'")?;
            TypeKind::List(child)
        }
        Some(TokenKind::Punctuation(Punctuation::LeftBrace)) => parse_record_type(parser, depth)?,
        _ => return Err(parser.expected("a type")),
    };
    let base_end = parser.previous_span();
    let base_depth = type_child_depth(parser, &base_kind).saturating_add(1);
    let base_span = join_span(start, base_end);
    if base_depth as usize > parser.limits.max_type_depth {
        return Err(parser.bound_error("type depth", parser.limits.max_type_depth, base_span));
    }
    let base_id = parser.add_reserved_type(Type {
        kind: base_kind,
        span: base_span,
        depth: base_depth,
    })?;
    if matches!(
        parser.current().map(|token| &token.kind),
        Some(TokenKind::Punctuation(Punctuation::Question))
    ) {
        let span = parser.current_span();
        let optional_depth = base_depth.saturating_add(1);
        if optional_depth as usize > parser.limits.max_type_depth {
            return Err(parser.bound_error("type depth", parser.limits.max_type_depth, span));
        }
        parser.reserve_type(span)?;
        parser.advance()?;
        let span = join_span(start, parser.previous_span());
        parser.add_reserved_type(Type {
            kind: TypeKind::Optional(base_id),
            span,
            depth: optional_depth,
        })
    } else {
        Ok(base_id)
    }
}

fn parse_record_type(
    parser: &mut Parser<'_>,
    depth: usize,
) -> Result<TypeKind, crate::api::Diagnostic> {
    parser.advance()?;
    let mut field_ids = Vec::new();
    if !parser.take_punctuation(Punctuation::RightBrace)? {
        loop {
            if field_ids.len() >= parser.limits.max_record_fields {
                return Err(parser.bound_error(
                    "record field count",
                    parser.limits.max_record_fields,
                    parser.current_span(),
                ));
            }
            let field_start = parser.current_span();
            parser.reserve_record_field(field_start)?;
            let (name, span) = parser.identifier()?;
            parser.expect_punctuation(Punctuation::Colon, "':'")?;
            let ty = parse_type(parser, depth.saturating_add(1))?;
            let field_depth = parser
                .module
                .ty(ty)
                .map_or(1, |node| node.depth)
                .saturating_add(1);
            let field = TypeField {
                name,
                ty,
                span: join_span(span, parser.previous_span()),
                depth: field_depth,
            };
            field_ids.push(parser.add_type_field(field)?);
            if parser.take_punctuation(Punctuation::Comma)? {
                if parser.take_punctuation(Punctuation::RightBrace)? {
                    break;
                }
            } else {
                parser.expect_punctuation(Punctuation::RightBrace, "'}'")?;
                break;
            }
        }
    }
    Ok(TypeKind::Record(field_ids))
}

fn type_child_depth(parser: &Parser<'_>, kind: &TypeKind) -> u32 {
    match kind {
        TypeKind::Primitive(_) => 0,
        TypeKind::List(id) | TypeKind::Optional(id) => {
            parser.module.ty(*id).map_or(0, |node| node.depth)
        }
        TypeKind::Record(fields) => fields
            .iter()
            .filter_map(|field_id| parser.module.type_field(*field_id).map(|field| field.depth))
            .max()
            .unwrap_or(0),
    }
}

fn primitive(keyword: Keyword) -> Option<PrimitiveType> {
    match keyword {
        Keyword::I32 => Some(PrimitiveType::I32),
        Keyword::I64 => Some(PrimitiveType::I64),
        Keyword::F64 => Some(PrimitiveType::F64),
        Keyword::Bool => Some(PrimitiveType::Bool),
        Keyword::String => Some(PrimitiveType::String),
        Keyword::Bytes => Some(PrimitiveType::Bytes),
        Keyword::Null => Some(PrimitiveType::Null),
        Keyword::Validation => Some(PrimitiveType::Validation),
        Keyword::Batch => Some(PrimitiveType::Batch),
        Keyword::Request => Some(PrimitiveType::Request),
        Keyword::Response => Some(PrimitiveType::Response),
        _ => None,
    }
}
