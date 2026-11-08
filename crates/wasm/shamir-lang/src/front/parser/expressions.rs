use crate::api::Diagnostic;
use crate::front::{IntegerLiteral, Keyword, Operator, Punctuation, TokenKind};

use super::super::ast::{BinaryOp, Expr, ExprId, ExprKind, UnaryOp, ValueField};
use super::parse::{join_span, Parser};

/// Parses one expression with conditional as the lowest precedence form.
pub(super) fn parse_expression(parser: &mut Parser<'_>) -> Result<ExprId, Diagnostic> {
    parser.ensure_node_capacity(parser.current_span())?;
    parse_conditional(parser)
}

fn parse_conditional(parser: &mut Parser<'_>) -> Result<ExprId, Diagnostic> {
    let condition = parse_or(parser)?;
    if !parser.take_punctuation(Punctuation::Question)? {
        return Ok(condition);
    }
    let start = expr_span(parser, condition);
    parser.reserve_expr(parser.current_span())?;
    parser.enter(parser.previous_span())?;
    let result = (|| {
        let then_value = parse_expression(parser)?;
        parser.expect_punctuation(Punctuation::Colon, "':'")?;
        let else_value = parse_conditional(parser)?;
        let span = join_span(start, expr_span(parser, else_value));
        let depth = child_depth(parser, &[condition, then_value, else_value]).saturating_add(1);
        parser.add_reserved_expr(Expr {
            kind: ExprKind::Conditional {
                condition,
                then_value,
                else_value,
            },
            span,
            depth,
        })
    })();
    parser.leave();
    result
}

fn parse_or(parser: &mut Parser<'_>) -> Result<ExprId, Diagnostic> {
    parse_left_chain(parser, parse_and, &[Operator::OrOr])
}

fn parse_and(parser: &mut Parser<'_>) -> Result<ExprId, Diagnostic> {
    parse_left_chain(parser, parse_equality, &[Operator::AndAnd])
}

fn parse_equality(parser: &mut Parser<'_>) -> Result<ExprId, Diagnostic> {
    let left = parse_comparison(parser)?;
    let Some(op) = peek_binary(parser, &[Operator::EqualEqual, Operator::BangEqual]) else {
        return Ok(left);
    };
    parser.reserve_expr(parser.current_span())?;
    parser.advance()?;
    let right = parse_comparison(parser)?;
    let result = make_binary(parser, left, right, op)?;
    if peek_binary(parser, &[Operator::EqualEqual, Operator::BangEqual]).is_some() {
        return Err(parser.syntax("equality operators do not chain"));
    }
    Ok(result)
}

fn parse_comparison(parser: &mut Parser<'_>) -> Result<ExprId, Diagnostic> {
    let left = parse_additive(parser)?;
    let Some(op) = peek_binary(
        parser,
        &[
            Operator::Less,
            Operator::LessEqual,
            Operator::Greater,
            Operator::GreaterEqual,
        ],
    ) else {
        return Ok(left);
    };
    parser.reserve_expr(parser.current_span())?;
    parser.advance()?;
    let right = parse_additive(parser)?;
    let result = make_binary(parser, left, right, op)?;
    if peek_binary(
        parser,
        &[
            Operator::Less,
            Operator::LessEqual,
            Operator::Greater,
            Operator::GreaterEqual,
        ],
    )
    .is_some()
    {
        return Err(parser.syntax("comparison operators do not chain"));
    }
    Ok(result)
}

fn parse_additive(parser: &mut Parser<'_>) -> Result<ExprId, Diagnostic> {
    parse_left_chain(
        parser,
        parse_multiplicative,
        &[Operator::Plus, Operator::Minus],
    )
}

fn parse_multiplicative(parser: &mut Parser<'_>) -> Result<ExprId, Diagnostic> {
    parse_left_chain(
        parser,
        parse_unary,
        &[Operator::Star, Operator::Slash, Operator::Percent],
    )
}

fn parse_left_chain(
    parser: &mut Parser<'_>,
    child: fn(&mut Parser<'_>) -> Result<ExprId, Diagnostic>,
    operators: &[Operator],
) -> Result<ExprId, Diagnostic> {
    let mut left = child(parser)?;
    loop {
        let Some(op) = peek_binary(parser, operators) else {
            return Ok(left);
        };
        parser.reserve_expr(parser.current_span())?;
        parser.advance()?;
        let right = child(parser)?;
        left = make_binary(parser, left, right, op)?;
    }
}

fn parse_unary(parser: &mut Parser<'_>) -> Result<ExprId, Diagnostic> {
    let mut prefixes = Vec::new();
    while let Some(op) = match parser.current().map(|token| &token.kind) {
        Some(TokenKind::Operator(Operator::Bang)) => Some(UnaryOp::Not),
        Some(TokenKind::Operator(Operator::Minus)) => Some(UnaryOp::Negate),
        Some(TokenKind::Operator(Operator::Plus)) => Some(UnaryOp::Positive),
        _ => None,
    } {
        let span = parser.current_span();
        parser.reserve_expr(span)?;
        let span = parser.advance()?.span;
        parser.enter(span)?;
        prefixes.push((op, span));
    }
    let allow_min = prefixes
        .last()
        .is_some_and(|(op, _)| *op == UnaryOp::Negate);
    let parsed = parse_postfix(parser, allow_min);
    for _ in 0..prefixes.len() {
        parser.leave();
    }
    let mut operand = parsed?;
    for (op, op_span) in prefixes.into_iter().rev() {
        let depth = child_depth(parser, &[operand]).saturating_add(1);
        let span = join_span(op_span, expr_span(parser, operand));
        operand = parser.add_reserved_expr(Expr {
            kind: ExprKind::Unary { op, operand },
            span,
            depth,
        })?;
    }
    Ok(operand)
}

fn parse_postfix(parser: &mut Parser<'_>, allow_min_candidate: bool) -> Result<ExprId, Diagnostic> {
    let mut expression = parse_primary(parser, allow_min_candidate)?;
    loop {
        let is_min_candidate = parser.module.expression(expression).is_some_and(|node| {
            matches!(
                node.kind,
                ExprKind::Integer {
                    negative_min_candidate: true,
                    ..
                }
            )
        });
        if is_min_candidate
            && matches!(
                parser.current().map(|token| &token.kind),
                Some(TokenKind::Punctuation(
                    Punctuation::LeftParen | Punctuation::LeftBracket | Punctuation::Dot
                ))
            )
        {
            return Err(Diagnostic::new(
                "L002",
                "minimum integer magnitude cannot have a postfix suffix",
                expr_span(parser, expression),
            ));
        }
        if parser.take_punctuation(Punctuation::LeftParen)? {
            let open = parser.previous_span();
            parser.reserve_expr(open)?;
            if !parser
                .module
                .expression(expression)
                .is_some_and(|node| matches!(&node.kind, ExprKind::Identifier(_)))
            {
                return Err(Diagnostic::new(
                    "P003",
                    "call target must be a direct identifier",
                    open,
                ));
            }
            parser.enter(open)?;
            let result = parse_call(parser, expression);
            parser.leave();
            expression = result?;
        } else if parser.take_punctuation(Punctuation::Dot)? {
            parser.reserve_expr(parser.previous_span())?;
            let (name, _) = parser.identifier()?;
            let span = join_span(expr_span(parser, expression), parser.previous_span());
            let depth = child_depth(parser, &[expression]).saturating_add(1);
            expression = parser.add_reserved_expr(Expr {
                kind: ExprKind::Field {
                    target: expression,
                    name,
                },
                span,
                depth,
            })?;
        } else if parser.take_punctuation(Punctuation::LeftBracket)? {
            let open = parser.previous_span();
            parser.reserve_expr(open)?;
            parser.enter(open)?;
            let result = (|| {
                let index = parse_expression(parser)?;
                parser.expect_punctuation(Punctuation::RightBracket, "']'")?;
                let span = join_span(expr_span(parser, expression), parser.previous_span());
                let depth = child_depth(parser, &[expression, index]).saturating_add(1);
                parser.add_reserved_expr(Expr {
                    kind: ExprKind::Index {
                        target: expression,
                        index,
                    },
                    span,
                    depth,
                })
            })();
            parser.leave();
            expression = result?;
        } else {
            return Ok(expression);
        }
    }
}

fn parse_call(parser: &mut Parser<'_>, target: ExprId) -> Result<ExprId, Diagnostic> {
    let mut arguments = Vec::new();
    if !parser.take_punctuation(Punctuation::RightParen)? {
        loop {
            arguments.push(parse_expression(parser)?);
            if parser.take_punctuation(Punctuation::Comma)? {
                if parser.take_punctuation(Punctuation::RightParen)? {
                    break;
                }
            } else {
                parser.expect_punctuation(Punctuation::RightParen, "')'")?;
                break;
            }
        }
    }
    let span = join_span(expr_span(parser, target), parser.previous_span());
    let depth = child_depth_many(
        parser,
        std::iter::once(target).chain(arguments.iter().copied()),
    )
    .saturating_add(1);
    parser.add_reserved_expr(Expr {
        kind: ExprKind::Call { target, arguments },
        span,
        depth,
    })
}

fn parse_primary(parser: &mut Parser<'_>, allow_min_candidate: bool) -> Result<ExprId, Diagnostic> {
    let span = parser.current_span();
    let is_simple = parser.current().is_some_and(|token| {
        matches!(
            &token.kind,
            TokenKind::Identifier(_)
                | TokenKind::Integer(_)
                | TokenKind::Float(_)
                | TokenKind::String(_)
                | TokenKind::Bytes(_)
                | TokenKind::Keyword(Keyword::True | Keyword::False | Keyword::Null)
        )
    });
    if is_simple {
        let span = parser.current_span();
        parser.reserve_expr(span)?;
        let token = parser.advance()?;
        let kind = match token.kind {
            TokenKind::Identifier(name) => ExprKind::Identifier(name),
            TokenKind::Integer(IntegerLiteral {
                magnitude,
                suffix,
                radix,
                negative_min_candidate,
            }) => {
                if negative_min_candidate && !allow_min_candidate {
                    return Err(Diagnostic::new(
                        "L002",
                        "integer magnitude is valid only as the direct operand of unary minus",
                        token.span,
                    ));
                }
                ExprKind::Integer {
                    magnitude,
                    suffix,
                    radix,
                    negative_min_candidate,
                }
            }
            TokenKind::Float(value) => ExprKind::Float(value),
            TokenKind::String(value) => ExprKind::String(value),
            TokenKind::Bytes(value) => ExprKind::Bytes(value),
            TokenKind::Keyword(Keyword::True) => ExprKind::Boolean(true),
            TokenKind::Keyword(Keyword::False) => ExprKind::Boolean(false),
            TokenKind::Keyword(Keyword::Null) => ExprKind::Null,
            _ => return Err(parser.syntax("expected an expression")),
        };
        return parser.add_reserved_expr(Expr {
            kind,
            span: token.span,
            depth: 1,
        });
    }
    match parser.current().map(|token| &token.kind) {
        Some(TokenKind::Punctuation(Punctuation::LeftParen)) => {
            parser.reserve_expr(span)?;
            parser.advance()?;
            parser.enter(span)?;
            let result = (|| {
                let inner = parse_expression(parser)?;
                parser.expect_punctuation(Punctuation::RightParen, "')'")?;
                let group_span = join_span(span, parser.previous_span());
                let depth = child_depth(parser, &[inner]).saturating_add(1);
                parser.add_reserved_expr(Expr {
                    kind: ExprKind::Group(inner),
                    span: group_span,
                    depth,
                })
            })();
            parser.leave();
            result
        }
        Some(TokenKind::Punctuation(Punctuation::LeftBracket)) => parse_list(parser, span),
        Some(TokenKind::Punctuation(Punctuation::LeftBrace)) => parse_record(parser, span),
        _ => Err(parser.syntax("expected an expression")),
    }
}

fn parse_list(parser: &mut Parser<'_>, start: crate::api::Span) -> Result<ExprId, Diagnostic> {
    parser.reserve_expr(start)?;
    parser.advance()?;
    parser.enter(start)?;
    let result = (|| {
        let mut elements = Vec::new();
        if !parser.take_punctuation(Punctuation::RightBracket)? {
            loop {
                elements.push(parse_expression(parser)?);
                if parser.take_punctuation(Punctuation::Comma)? {
                    if parser.take_punctuation(Punctuation::RightBracket)? {
                        break;
                    }
                } else {
                    parser.expect_punctuation(Punctuation::RightBracket, "']'")?;
                    break;
                }
            }
        }
        let span = join_span(start, parser.previous_span());
        let depth = child_depth_many(parser, elements.iter().copied()).saturating_add(1);
        parser.add_reserved_expr(Expr {
            kind: ExprKind::List(elements),
            span,
            depth,
        })
    })();
    parser.leave();
    result
}

fn parse_record(parser: &mut Parser<'_>, start: crate::api::Span) -> Result<ExprId, Diagnostic> {
    parser.reserve_expr(start)?;
    parser.advance()?;
    parser.enter(start)?;
    let result = (|| {
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
                parser.reserve_record_field(parser.current_span())?;
                let (name, field_start) = parser.identifier()?;
                parser.expect_punctuation(Punctuation::Colon, "':'")?;
                let value = parse_expression(parser)?;
                let field = ValueField {
                    name,
                    value,
                    span: join_span(field_start, expr_span(parser, value)),
                    depth: parser
                        .module
                        .expression(value)
                        .map_or(1, |node| node.depth)
                        .saturating_add(1),
                };
                let field_id = parser.add_value_field(field)?;
                field_ids.push(field_id);
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
        let span = join_span(start, parser.previous_span());
        let depth = field_ids
            .iter()
            .filter_map(|id| parser.module.value_field(*id).map(|field| field.depth))
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        parser.add_reserved_expr(Expr {
            kind: ExprKind::Record(field_ids),
            span,
            depth,
        })
    })();
    parser.leave();
    result
}

fn make_binary(
    parser: &mut Parser<'_>,
    left: ExprId,
    right: ExprId,
    op: BinaryOp,
) -> Result<ExprId, Diagnostic> {
    let span = join_span(expr_span(parser, left), expr_span(parser, right));
    let depth = child_depth(parser, &[left, right]).saturating_add(1);
    parser.add_reserved_expr(Expr {
        kind: ExprKind::Binary { op, left, right },
        span,
        depth,
    })
}

fn peek_binary(parser: &Parser<'_>, operators: &[Operator]) -> Option<BinaryOp> {
    let Some(TokenKind::Operator(operator)) = parser.current().map(|token| &token.kind) else {
        return None;
    };
    if !operators.contains(operator) {
        return None;
    }
    Some(match operator {
        Operator::Star => BinaryOp::Multiply,
        Operator::Slash => BinaryOp::Divide,
        Operator::Percent => BinaryOp::Remainder,
        Operator::Plus => BinaryOp::Add,
        Operator::Minus => BinaryOp::Subtract,
        Operator::Less => BinaryOp::Less,
        Operator::LessEqual => BinaryOp::LessEqual,
        Operator::Greater => BinaryOp::Greater,
        Operator::GreaterEqual => BinaryOp::GreaterEqual,
        Operator::EqualEqual => BinaryOp::Equal,
        Operator::BangEqual => BinaryOp::NotEqual,
        Operator::AndAnd => BinaryOp::And,
        Operator::OrOr => BinaryOp::Or,
        _ => return None,
    })
}

fn expr_span(parser: &Parser<'_>, id: ExprId) -> crate::api::Span {
    parser
        .module
        .expression(id)
        .map_or_else(|| parser.current_span(), |node| node.span)
}

fn child_depth(parser: &Parser<'_>, children: &[ExprId]) -> u32 {
    children
        .iter()
        .filter_map(|id| parser.module.expression(*id).map(|node| node.depth))
        .max()
        .unwrap_or(0)
}

fn child_depth_many(parser: &Parser<'_>, children: impl Iterator<Item = ExprId>) -> u32 {
    children
        .filter_map(|id| parser.module.expression(id).map(|node| node.depth))
        .max()
        .unwrap_or(0)
}
