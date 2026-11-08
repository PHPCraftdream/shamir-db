use crate::api::Diagnostic;
use crate::front::{Keyword, Operator, Punctuation, TokenKind};

use super::super::ast::{ExprId, ExprKind, ForKind, Statement, StatementId, StatementKind};
use super::parse::{join_span, Parser};

/// Parses a block, preserving all child statements by arena IDs.
pub(super) fn parse_block(parser: &mut Parser<'_>) -> Result<StatementId, Diagnostic> {
    parser.reserve_statement(parser.current_span())?;
    let open = parser.expect_punctuation(Punctuation::LeftBrace, "'{'")?;
    parser.enter(open.span)?;
    let result = (|| {
        let mut children = Vec::new();
        while !matches!(
            parser.current().map(|token| &token.kind),
            Some(TokenKind::Punctuation(Punctuation::RightBrace))
        ) {
            if parser.at_eof() {
                return Err(parser.expected("'}'"));
            }
            children.push(parse_statement(parser)?);
        }
        parser.expect_punctuation(Punctuation::RightBrace, "'}'")?;
        let span = join_span(open.span, parser.previous_span());
        let depth = child_statement_depth(parser, &children).saturating_add(1);
        parser.add_reserved_statement(Statement {
            kind: StatementKind::Block(children),
            span,
            depth,
        })
    })();
    parser.leave();
    result
}

fn parse_statement(parser: &mut Parser<'_>) -> Result<StatementId, Diagnostic> {
    match parser.current().map(|token| &token.kind) {
        Some(TokenKind::Keyword(Keyword::Let | Keyword::Const)) => parse_variable(parser),
        Some(TokenKind::Keyword(Keyword::If)) => parse_if(parser),
        Some(TokenKind::Keyword(Keyword::While)) => parse_while(parser),
        Some(TokenKind::Keyword(Keyword::For)) => parse_for(parser),
        Some(TokenKind::Keyword(Keyword::Return)) => parse_return(parser),
        Some(TokenKind::Keyword(Keyword::Break)) => {
            parse_control(parser, Keyword::Break, StatementKind::Break)
        }
        Some(TokenKind::Keyword(Keyword::Continue)) => {
            parse_control(parser, Keyword::Continue, StatementKind::Continue)
        }
        Some(TokenKind::Keyword(Keyword::Throw)) => parse_throw(parser),
        Some(TokenKind::Punctuation(Punctuation::LeftBrace)) => {
            if starts_record_literal(parser) {
                parse_expression_or_assignment(parser)
            } else {
                parse_block(parser)
            }
        }
        Some(TokenKind::Punctuation(Punctuation::RightBrace)) | Some(TokenKind::Eof) | None => {
            Err(parser.syntax("expected a statement"))
        }
        _ => parse_expression_or_assignment(parser),
    }
}

/// `{ name :` can only open a record literal (a block never starts with a
/// label), so such a statement is an expression statement.
fn starts_record_literal(parser: &Parser<'_>) -> bool {
    matches!(
        parser.peek(1).map(|token| &token.kind),
        Some(TokenKind::Identifier(_))
    ) && matches!(
        parser.peek(2).map(|token| &token.kind),
        Some(TokenKind::Punctuation(Punctuation::Colon))
    )
}

fn parse_variable(parser: &mut Parser<'_>) -> Result<StatementId, Diagnostic> {
    if parser.locals >= parser.limits.max_locals_per_function {
        return Err(parser.bound_error(
            "local count",
            parser.limits.max_locals_per_function,
            parser.current_span(),
        ));
    }
    parser.reserve_statement(parser.current_span())?;
    let keyword = parser.advance()?;
    let mutable = matches!(keyword.kind, TokenKind::Keyword(Keyword::Let));
    let (name, _) = parser.identifier()?;
    let ty = if parser.take_punctuation(Punctuation::Colon)? {
        Some(parser.parse_type()?)
    } else {
        None
    };
    parser.expect_operator(Operator::Assign, "'='")?;
    let value = parser.parse_expression()?;
    parser.expect_punctuation(Punctuation::Semicolon, "';'")?;
    parser.locals = parser.locals.saturating_add(1);
    let span = join_span(keyword.span, parser.previous_span());
    let depth = child_expr_depth(parser, &[value])
        .max(
            ty.and_then(|id| parser.module.ty(id).map(|node| node.depth))
                .unwrap_or(0),
        )
        .saturating_add(1);
    parser.add_reserved_statement(Statement {
        kind: StatementKind::Let {
            mutable,
            name,
            ty,
            value,
        },
        span,
        depth,
    })
}

fn parse_if(parser: &mut Parser<'_>) -> Result<StatementId, Diagnostic> {
    parser.reserve_statement(parser.current_span())?;
    let keyword = parser.advance()?;
    parser.expect_punctuation(Punctuation::LeftParen, "'('")?;
    let condition = parser.parse_expression()?;
    parser.expect_punctuation(Punctuation::RightParen, "')'")?;
    let then_block = parser.parse_block()?;
    let else_branch = if parser.take_keyword(Keyword::Else)? {
        match parser.current().map(|token| &token.kind) {
            Some(TokenKind::Keyword(Keyword::If)) => {
                parser.enter(parser.current_span())?;
                let result = parse_if(parser);
                parser.leave();
                Some(result?)
            }
            Some(TokenKind::Punctuation(Punctuation::LeftBrace)) => Some(parser.parse_block()?),
            _ => return Err(parser.expected("a block or if after else")),
        }
    } else {
        None
    };
    let span = join_span(keyword.span, parser.previous_span());
    let depth = child_expr_depth(parser, &[condition])
        .saturating_add(1)
        .max(statement_depth(parser, then_block).saturating_add(1))
        .max(else_branch.map_or(0, |branch| {
            statement_depth(parser, branch).saturating_add(1)
        }));
    parser.add_reserved_statement(Statement {
        kind: StatementKind::If {
            condition,
            then_block,
            else_branch,
        },
        span,
        depth,
    })
}

fn parse_while(parser: &mut Parser<'_>) -> Result<StatementId, Diagnostic> {
    parser.reserve_statement(parser.current_span())?;
    let keyword = parser.advance()?;
    parser.expect_punctuation(Punctuation::LeftParen, "'('")?;
    let condition = parser.parse_expression()?;
    parser.expect_punctuation(Punctuation::RightParen, "')'")?;
    let body = parser.parse_block()?;
    let span = join_span(keyword.span, parser.previous_span());
    let depth = child_expr_depth(parser, &[condition])
        .saturating_add(1)
        .max(statement_depth(parser, body).saturating_add(1));
    parser.add_reserved_statement(Statement {
        kind: StatementKind::While { condition, body },
        span,
        depth,
    })
}

fn parse_for(parser: &mut Parser<'_>) -> Result<StatementId, Diagnostic> {
    let keyword = parser.advance()?;
    parser.expect_punctuation(Punctuation::LeftParen, "'('")?;
    let binding_span = parser.current_span();
    if parser.locals >= parser.limits.max_locals_per_function {
        return Err(parser.bound_error(
            "local count",
            parser.limits.max_locals_per_function,
            binding_span,
        ));
    }
    parser.reserve_statement(binding_span)?;
    let (name, _) = parser.identifier()?;
    parser.locals = parser.locals.saturating_add(1);
    parser.expect_keyword(Keyword::In, "'in'")?;
    let sequence = parser.parse_expression()?;
    let (kind, end) = if parser.take_operator(Operator::Range)? {
        (ForKind::Range, Some(parser.parse_expression()?))
    } else {
        (ForKind::List, None)
    };
    parser.expect_punctuation(Punctuation::RightParen, "')'")?;
    let body = parser.parse_block()?;
    let span = join_span(keyword.span, parser.previous_span());
    let depth = child_expr_depth(parser, &[sequence])
        .saturating_add(1)
        .max(end.map_or(0, |id| child_expr_depth(parser, &[id]).saturating_add(1)))
        .max(statement_depth(parser, body).saturating_add(1));
    parser.add_reserved_statement(Statement {
        kind: StatementKind::For {
            kind,
            name,
            start_or_list: sequence,
            end,
            body,
        },
        span,
        depth,
    })
}

fn parse_return(parser: &mut Parser<'_>) -> Result<StatementId, Diagnostic> {
    parser.reserve_statement(parser.current_span())?;
    let keyword = parser.advance()?;
    let value = if matches!(
        parser.current().map(|token| &token.kind),
        Some(TokenKind::Punctuation(Punctuation::Semicolon))
    ) {
        None
    } else {
        Some(parser.parse_expression()?)
    };
    parser.expect_punctuation(Punctuation::Semicolon, "';'")?;
    let span = join_span(keyword.span, parser.previous_span());
    let depth = value.map_or(1, |id| child_expr_depth(parser, &[id]).saturating_add(1));
    parser.add_reserved_statement(Statement {
        kind: StatementKind::Return(value),
        span,
        depth,
    })
}

fn parse_control(
    parser: &mut Parser<'_>,
    keyword: Keyword,
    kind: StatementKind,
) -> Result<StatementId, Diagnostic> {
    parser.reserve_statement(parser.current_span())?;
    let name = match keyword {
        Keyword::Break => "'break'",
        _ => "'continue'",
    };
    let token = parser.expect_keyword(keyword, name)?;
    parser.expect_punctuation(Punctuation::Semicolon, "';'")?;
    parser.add_reserved_statement(Statement {
        kind,
        span: join_span(token.span, parser.previous_span()),
        depth: 1,
    })
}

fn parse_throw(parser: &mut Parser<'_>) -> Result<StatementId, Diagnostic> {
    parser.reserve_statement(parser.current_span())?;
    let keyword = parser.advance()?;
    let value = parser.parse_expression()?;
    parser.expect_punctuation(Punctuation::Semicolon, "';'")?;
    let span = join_span(keyword.span, parser.previous_span());
    let depth = child_expr_depth(parser, &[value]).saturating_add(1);
    parser.add_reserved_statement(Statement {
        kind: StatementKind::Throw(value),
        span,
        depth,
    })
}

fn parse_expression_or_assignment(parser: &mut Parser<'_>) -> Result<StatementId, Diagnostic> {
    let span = parser.current_span();
    parser.reserve_statement(span)?;
    let expression = parser.parse_expression()?;
    if parser.take_operator(Operator::Assign)? {
        if !is_lvalue(parser, expression) {
            return Err(Diagnostic::new(
                "P003",
                "assignment target must be an identifier, field, or index",
                expr_span(parser, expression),
            ));
        }
        let value = parser.parse_expression()?;
        parser.expect_punctuation(Punctuation::Semicolon, "';'")?;
        let span = join_span(expr_span(parser, expression), parser.previous_span());
        let depth = child_expr_depth(parser, &[expression, value]).saturating_add(1);
        parser.add_reserved_statement(Statement {
            kind: StatementKind::Assign {
                target: expression,
                value,
            },
            span,
            depth,
        })
    } else {
        parser.expect_punctuation(Punctuation::Semicolon, "';'")?;
        let span = join_span(expr_span(parser, expression), parser.previous_span());
        let depth = child_expr_depth(parser, &[expression]).saturating_add(1);
        parser.add_reserved_statement(Statement {
            kind: StatementKind::Expression(expression),
            span,
            depth,
        })
    }
}

fn is_lvalue(parser: &Parser<'_>, id: ExprId) -> bool {
    parser.module.expression(id).is_some_and(|node| {
        matches!(
            &node.kind,
            ExprKind::Identifier(_) | ExprKind::Field { .. } | ExprKind::Index { .. }
        )
    })
}

fn expr_span(parser: &Parser<'_>, id: ExprId) -> crate::api::Span {
    parser
        .module
        .expression(id)
        .map_or(parser.current_span(), |node| node.span)
}

fn child_expr_depth(parser: &Parser<'_>, ids: &[ExprId]) -> u32 {
    ids.iter()
        .filter_map(|id| parser.module.expression(*id).map(|node| node.depth))
        .max()
        .unwrap_or(0)
}

fn child_statement_depth(parser: &Parser<'_>, ids: &[StatementId]) -> u32 {
    ids.iter()
        .filter_map(|id| parser.module.statement(*id).map(|node| node.depth))
        .max()
        .unwrap_or(0)
}

fn statement_depth(parser: &Parser<'_>, id: StatementId) -> u32 {
    parser.module.statement(id).map_or(0, |node| node.depth)
}
