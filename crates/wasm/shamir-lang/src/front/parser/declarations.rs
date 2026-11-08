use crate::api::Diagnostic;
use crate::front::{Keyword, Operator, Punctuation, TokenKind};

use super::super::ast::{EntryKind, Function, Parameter};
use super::parse::{join_span, Parser};

/// Parses a function declaration and requires entry annotations to be unique.
pub(super) fn parse_function(parser: &mut Parser<'_>) -> Result<(), Diagnostic> {
    let start = parser.current_span();
    let local_scope = parser.locals;
    parser.reserve_function_node(start)?;
    let entry_kind = if matches!(
        parser.current().map(|token| &token.kind),
        Some(TokenKind::AnnotationEntry)
    ) {
        let annotation = parser.advance()?.span;
        if let Some(first) = parser.entry_span {
            return Err(Diagnostic::new(
                "P002",
                "module has more than one @entry declaration",
                annotation,
            )
            .with_related(first, "first @entry declaration"));
        }
        let kind = match parser.current().map(|token| &token.kind) {
            Some(TokenKind::Keyword(Keyword::Scalar)) => EntryKind::Scalar,
            Some(TokenKind::Keyword(Keyword::Validator)) => EntryKind::Validator,
            Some(TokenKind::Keyword(Keyword::Procedure)) => EntryKind::Procedure,
            _ => return Err(parser.expected("scalar, validator, or procedure")),
        };
        parser.advance()?;
        parser.entry_span = Some(annotation);
        Some(kind)
    } else if parser.take_keyword(Keyword::Fn)? {
        None
    } else if matches!(
        parser.current().map(|token| &token.kind),
        Some(TokenKind::Keyword(
            Keyword::Scalar | Keyword::Validator | Keyword::Procedure
        ))
    ) {
        return Err(Diagnostic::new(
            "P002",
            "unmarked declaration; expected fn or @entry",
            parser.current_span(),
        ));
    } else {
        return Err(parser.expected("fn or @entry"));
    };
    let (name, _) = parser.identifier()?;
    parser.expect_punctuation(Punctuation::LeftParen, "'('")?;
    parser.enter(parser.previous_span())?;
    let parameters = parse_parameters(parser)?;
    parser.expect_punctuation(Punctuation::RightParen, "')'")?;
    parser.leave();
    parser.expect_operator(Operator::Arrow, "'->'")?;
    let return_type = parser.parse_type()?;
    parser.locals = 0;
    let body = parser.parse_block()?;
    parser.locals = local_scope;
    let end = parser.previous_span();
    let mut depth = parser.module.statement(body).map_or(1, |node| node.depth);
    depth = depth.max(parser.module.ty(return_type).map_or(1, |node| node.depth));
    for parameter in &parameters {
        depth = depth.max(
            parser
                .module
                .parameter(*parameter)
                .map_or(1, |node| node.depth),
        );
    }
    depth = depth.saturating_add(1);
    parser.add_function(Function {
        name,
        entry_kind,
        parameters,
        return_type,
        body,
        span: join_span(start, end),
        depth,
    })
}

fn parse_parameters(
    parser: &mut Parser<'_>,
) -> Result<Vec<super::super::ast::ParameterId>, Diagnostic> {
    let mut parameters = Vec::new();
    if matches!(
        parser.current().map(|token| &token.kind),
        Some(TokenKind::Punctuation(Punctuation::RightParen))
    ) {
        return Ok(parameters);
    }
    loop {
        if parameters.len() >= parser.limits.max_parameters {
            return Err(parser.bound_error(
                "parameter count",
                parser.limits.max_parameters,
                parser.current_span(),
            ));
        }
        parser.reserve_parameter(parser.current_span())?;
        let (name, span) = parser.identifier()?;
        parser.expect_punctuation(Punctuation::Colon, "':'")?;
        let ty = parser.parse_type()?;
        let parameter_span = join_span(span, parser.previous_span());
        let parameter = Parameter {
            name,
            ty,
            span: parameter_span,
            depth: parser
                .module
                .ty(ty)
                .map_or(1, |node| node.depth)
                .saturating_add(1),
        };
        let id = parser.add_parameter(parameter)?;
        parameters.push(id);
        if parser.take_punctuation(Punctuation::Comma)? {
            if matches!(
                parser.current().map(|token| &token.kind),
                Some(TokenKind::Punctuation(Punctuation::RightParen))
            ) {
                break;
            }
        } else {
            break;
        }
    }
    Ok(parameters)
}
