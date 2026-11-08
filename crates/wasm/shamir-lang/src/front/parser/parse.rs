use crate::api::{CompileLimits, Diagnostic, Span};
use crate::front::lexer::scan_with_budget;
use crate::front::{CompileBudget, Keyword, Operator, Punctuation, Token, TokenKind};

use super::super::ast::{
    Expr, ExprId, Function, FunctionId, Module, ParameterId, Statement, StatementId, Type,
    TypeFieldId, TypeId, ValueFieldId,
};
use super::{declarations, expressions, statements};

/// Parses source text into a module with its own compile budget.
pub fn parse(source: &str, limits: &CompileLimits) -> Result<Module, Diagnostic> {
    let limits = normalized_limits(limits)?;
    let mut budget = CompileBudget::new(&limits).map_err(invalid_limit)?;
    parse_scoped(source, &limits, &mut budget)
}

/// Parses source text through the scanner with the CALLER's compile budget
/// (one pipeline budget; `parse` is a thin wrapper creating its own).
pub fn parse_scoped(
    source: &str,
    limits: &CompileLimits,
    budget: &mut CompileBudget,
) -> Result<Module, Diagnostic> {
    let limits = normalized_limits(limits)?;
    if source.len() > limits.max_source_bytes {
        return Err(Diagnostic::new(
            "B001",
            format!(
                "source byte limit exceeded (limit {}, observed {})",
                limits.max_source_bytes,
                source.len()
            ),
            Span::source_start(),
        ));
    }
    let tokens = scan_with_budget(source, &limits, budget)?;
    parse_tokens_with_budget(&tokens, limits, budget)
}

/// Parses a complete token stream. Scanner-produced streams include EOF.
pub fn parse_tokens(tokens: &[Token], limits: &CompileLimits) -> Result<Module, Diagnostic> {
    let limits = normalized_limits(limits)?;
    if tokens.len() > limits.max_tokens {
        return Err(Diagnostic::new(
            "B001",
            format!("token count limit exceeded (limit {})", limits.max_tokens),
            tokens
                .get(limits.max_tokens)
                .map_or(Span::source_start(), |token| token.span),
        ));
    }
    let mut budget = CompileBudget::new(&limits).map_err(invalid_limit)?;
    parse_tokens_with_budget(tokens, limits, &mut budget)
}

fn normalized_limits(limits: &CompileLimits) -> Result<CompileLimits, Diagnostic> {
    limits.clamped().map_err(invalid_limit)
}

fn invalid_limit(error: crate::api::LimitError) -> Diagnostic {
    Diagnostic::new(
        "B001",
        format!("invalid zero compiler limit: {}", error.field),
        Span::source_start(),
    )
}

fn parse_tokens_with_budget(
    tokens: &[Token],
    limits: CompileLimits,
    budget: &mut CompileBudget,
) -> Result<Module, Diagnostic> {
    if tokens.is_empty() {
        return Err(Diagnostic::new(
            "P001",
            "expected end of input marker at the end of token stream",
            Span::source_start(),
        ));
    }
    if tokens.len() > limits.max_tokens {
        return Err(Diagnostic::new(
            "B001",
            format!("token count limit exceeded (limit {})", limits.max_tokens),
            tokens
                .get(limits.max_tokens)
                .map_or(Span::source_start(), |token| token.span),
        ));
    }
    let count = u64::try_from(tokens.len()).map_err(|_| {
        Diagnostic::new(
            "B001",
            "token count exceeds fuel range",
            Span::source_start(),
        )
    })?;
    budget.charge(
        count,
        tokens
            .first()
            .map_or(Span::source_start(), |token| token.span),
    )?;
    let eof_position = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Eof));
    if eof_position != tokens.len().checked_sub(1) {
        let span = eof_position.map_or_else(
            || {
                tokens
                    .last()
                    .map_or(Span::source_start(), |token| token.span)
            },
            |index| {
                tokens
                    .get(index)
                    .map_or(Span::source_start(), |token| token.span)
            },
        );
        return Err(Diagnostic::new(
            "P001",
            "expected end of input marker at the end of token stream",
            span,
        ));
    }
    let first = tokens
        .first()
        .map_or(Span::source_start(), |token| token.span);
    Parser {
        tokens,
        cursor: 0,
        limits,
        budget,
        module: Module::empty(first),
        nodes: 0,
        nesting: 0,
        locals: 0,
        entry_span: None,
    }
    .run()
}

pub(super) struct Parser<'a> {
    pub(super) tokens: &'a [Token],
    pub(super) cursor: usize,
    pub(super) limits: CompileLimits,
    pub(super) budget: &'a mut CompileBudget,
    pub(super) module: Module,
    pub(super) nodes: usize,
    pub(super) nesting: usize,
    pub(super) locals: usize,
    pub(super) entry_span: Option<Span>,
}

impl Parser<'_> {
    fn run(mut self) -> Result<Module, Diagnostic> {
        while !self.at_eof() {
            if self.module.functions.len() >= self.limits.max_functions {
                return Err(self.bound_error(
                    "function count",
                    self.limits.max_functions,
                    self.current_span(),
                ));
            }
            declarations::parse_function(&mut self)?;
        }
        if self.entry_span.is_none() {
            return Err(Diagnostic::new(
                "P002",
                "module requires exactly one @entry declaration",
                self.current_span(),
            ));
        }
        self.module.span = join_span(self.module.span, self.current_span());
        Ok(self.module)
    }

    pub(super) fn current(&self) -> Option<&Token> {
        self.tokens.get(self.cursor)
    }

    /// Token `offset` positions after the current one.
    pub(super) fn peek(&self, offset: usize) -> Option<&Token> {
        self.cursor
            .checked_add(offset)
            .and_then(|index| self.tokens.get(index))
    }

    pub(super) fn current_span(&self) -> Span {
        self.current().map_or_else(
            || {
                self.tokens
                    .last()
                    .map_or(Span::source_start(), |token| token.span)
            },
            |token| token.span,
        )
    }

    pub(super) fn previous_span(&self) -> Span {
        self.cursor
            .checked_sub(1)
            .and_then(|index| self.tokens.get(index))
            .map_or(self.current_span(), |token| token.span)
    }

    pub(super) fn at_eof(&self) -> bool {
        self.current()
            .is_none_or(|token| matches!(token.kind, TokenKind::Eof))
    }

    pub(super) fn advance(&mut self) -> Result<Token, Diagnostic> {
        let (span, payload_bytes) = {
            let token = self.current().ok_or_else(|| self.expected("a token"))?;
            let payload_bytes = match &token.kind {
                TokenKind::Identifier(value) | TokenKind::String(value) => value.len(),
                TokenKind::Bytes(value) => value.len(),
                _ => 0,
            };
            (token.span, payload_bytes)
        };
        let charge = u64::try_from(payload_bytes)
            .ok()
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or_else(|| self.bound_error("token payload work", u64::MAX as usize, span))?;
        self.budget.charge(charge, span)?;
        let token = self
            .current()
            .cloned()
            .ok_or_else(|| self.expected("a token"))?;
        if !matches!(token.kind, TokenKind::Eof) {
            self.cursor = self.cursor.saturating_add(1);
        }
        Ok(token)
    }

    pub(super) fn expected(&self, expected: &str) -> Diagnostic {
        Diagnostic::new(
            "P001",
            format!("expected {}, found {}", expected, self.found_description()),
            self.current_span(),
        )
    }

    pub(super) fn syntax(&self, message: &str) -> Diagnostic {
        Diagnostic::new("P003", message, self.current_span())
    }

    pub(super) fn found_description(&self) -> String {
        match self.current().map(|token| &token.kind) {
            None | Some(TokenKind::Eof) => "end of input".to_owned(),
            Some(TokenKind::Identifier(name)) => format!("identifier {}", bounded_name(name)),
            Some(TokenKind::Keyword(keyword)) => format!("keyword {}", keyword_name(*keyword)),
            Some(TokenKind::AnnotationEntry) => "@entry".to_owned(),
            Some(TokenKind::Integer(_)) => "integer literal".to_owned(),
            Some(TokenKind::Float(_)) => "float literal".to_owned(),
            Some(TokenKind::String(_)) => "string literal".to_owned(),
            Some(TokenKind::Bytes(_)) => "byte literal".to_owned(),
            Some(TokenKind::Operator(operator)) => format!("operator {}", operator_name(*operator)),
            Some(TokenKind::Punctuation(punctuation)) => {
                format!("'{}'", punctuation_name(*punctuation))
            }
        }
    }

    pub(super) fn take_keyword(&mut self, expected: Keyword) -> Result<bool, Diagnostic> {
        if matches!(self.current().map(|token| &token.kind), Some(TokenKind::Keyword(keyword)) if *keyword == expected)
        {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn take_operator(&mut self, expected: Operator) -> Result<bool, Diagnostic> {
        if matches!(self.current().map(|token| &token.kind), Some(TokenKind::Operator(operator)) if *operator == expected)
        {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn take_punctuation(&mut self, expected: Punctuation) -> Result<bool, Diagnostic> {
        if matches!(self.current().map(|token| &token.kind), Some(TokenKind::Punctuation(punctuation)) if *punctuation == expected)
        {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn expect_keyword(
        &mut self,
        expected: Keyword,
        name: &str,
    ) -> Result<Token, Diagnostic> {
        if matches!(self.current().map(|token| &token.kind), Some(TokenKind::Keyword(keyword)) if *keyword == expected)
        {
            self.advance()
        } else {
            Err(self.expected(name))
        }
    }

    pub(super) fn expect_operator(
        &mut self,
        expected: Operator,
        name: &str,
    ) -> Result<Token, Diagnostic> {
        if matches!(self.current().map(|token| &token.kind), Some(TokenKind::Operator(operator)) if *operator == expected)
        {
            self.advance()
        } else {
            Err(self.expected(name))
        }
    }

    pub(super) fn expect_punctuation(
        &mut self,
        expected: Punctuation,
        name: &str,
    ) -> Result<Token, Diagnostic> {
        if matches!(self.current().map(|token| &token.kind), Some(TokenKind::Punctuation(punctuation)) if *punctuation == expected)
        {
            self.advance()
        } else {
            Err(self.expected(name))
        }
    }

    pub(super) fn identifier(&mut self) -> Result<(String, Span), Diagnostic> {
        if !matches!(
            self.current().map(|token| &token.kind),
            Some(TokenKind::Identifier(_))
        ) {
            return Err(self.expected("an identifier"));
        }
        match self.advance()? {
            Token {
                kind: TokenKind::Identifier(name),
                span,
            } => Ok((name, span)),
            _ => Err(self.expected("an identifier")),
        }
    }

    pub(super) fn enter(&mut self, span: Span) -> Result<(), Diagnostic> {
        let next = self.nesting.saturating_add(1);
        if next > self.limits.max_ast_depth {
            return Err(self.bound_error("parser nesting depth", self.limits.max_ast_depth, span));
        }
        self.nesting = next;
        Ok(())
    }

    pub(super) fn leave(&mut self) {
        self.nesting = self.nesting.saturating_sub(1);
    }

    pub(super) fn ensure_node_capacity(&self, span: Span) -> Result<(), Diagnostic> {
        if self.nodes >= self.limits.max_ast_nodes {
            Err(self.bound_error("AST node count", self.limits.max_ast_nodes, span))
        } else {
            Ok(())
        }
    }

    pub(super) fn charge_node(&mut self, depth: u32, span: Span) -> Result<(), Diagnostic> {
        if depth as usize > self.limits.max_ast_depth {
            return Err(self.bound_error("AST depth", self.limits.max_ast_depth, span));
        }
        let next = self.nodes.saturating_add(1);
        if next > self.limits.max_ast_nodes {
            return Err(self.bound_error("AST node count", self.limits.max_ast_nodes, span));
        }
        self.budget.charge(1, span)?;
        self.nodes = next;
        Ok(())
    }

    pub(super) fn reserve_expr(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.charge_node(1, span)
    }

    pub(super) fn add_reserved_expr(&mut self, node: Expr) -> Result<ExprId, Diagnostic> {
        if node.depth as usize > self.limits.max_ast_depth {
            return Err(self.bound_error("AST depth", self.limits.max_ast_depth, node.span));
        }
        let id = u32::try_from(self.module.expressions.len())
            .map_err(|_| self.bound_error("expression arena", u32::MAX as usize, node.span))?;
        self.module.expressions.push(node);
        Ok(ExprId(id))
    }

    pub(super) fn add_reserved_statement(
        &mut self,
        node: Statement,
    ) -> Result<StatementId, Diagnostic> {
        if node.depth as usize > self.limits.max_ast_depth {
            return Err(self.bound_error("AST depth", self.limits.max_ast_depth, node.span));
        }
        let id = u32::try_from(self.module.statements.len())
            .map_err(|_| self.bound_error("statement arena", u32::MAX as usize, node.span))?;
        self.module.statements.push(node);
        Ok(StatementId(id))
    }

    pub(super) fn reserve_type(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.charge_node(1, span)
    }

    pub(super) fn add_reserved_type(&mut self, node: Type) -> Result<TypeId, Diagnostic> {
        if node.depth as usize > self.limits.max_ast_depth {
            return Err(self.bound_error("AST depth", self.limits.max_ast_depth, node.span));
        }
        let id = u32::try_from(self.module.types.len())
            .map_err(|_| self.bound_error("type arena", u32::MAX as usize, node.span))?;
        self.module.types.push(node);
        Ok(TypeId(id))
    }

    pub(super) fn reserve_function_node(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.charge_node(1, span)
    }

    pub(super) fn add_function(&mut self, function: Function) -> Result<(), Diagnostic> {
        if function.depth as usize > self.limits.max_ast_depth {
            return Err(self.bound_error("AST depth", self.limits.max_ast_depth, function.span));
        }
        let id = u32::try_from(self.module.functions.len())
            .map_err(|_| self.bound_error("function arena", u32::MAX as usize, function.span))?;
        self.module.functions.push(function);
        self.module.function_ids.push(FunctionId(id));
        Ok(())
    }

    pub(super) fn reserve_parameter(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.charge_node(1, span)
    }

    pub(super) fn add_parameter(
        &mut self,
        parameter: crate::front::ast::Parameter,
    ) -> Result<ParameterId, Diagnostic> {
        if parameter.depth as usize > self.limits.max_ast_depth {
            return Err(self.bound_error("AST depth", self.limits.max_ast_depth, parameter.span));
        }
        let id = u32::try_from(self.module.parameters.len())
            .map_err(|_| self.bound_error("parameter arena", u32::MAX as usize, parameter.span))?;
        self.module.parameters.push(parameter);
        let parameter_id = ParameterId(id);
        self.module.parameter_ids.push(parameter_id);
        Ok(parameter_id)
    }

    pub(super) fn reserve_record_field(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.charge_node(1, span)
    }

    pub(super) fn reserve_statement(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.charge_node(1, span)
    }

    pub(super) fn add_type_field(
        &mut self,
        field: crate::front::ast::TypeField,
    ) -> Result<TypeFieldId, Diagnostic> {
        if field.depth as usize > self.limits.max_ast_depth {
            return Err(self.bound_error("AST depth", self.limits.max_ast_depth, field.span));
        }
        let id = u32::try_from(self.module.type_fields.len())
            .map_err(|_| self.bound_error("type field arena", u32::MAX as usize, field.span))?;
        self.module.type_fields.push(field);
        let field_id = TypeFieldId(id);
        self.module.type_field_ids.push(field_id);
        Ok(field_id)
    }

    pub(super) fn add_value_field(
        &mut self,
        field: crate::front::ast::ValueField,
    ) -> Result<ValueFieldId, Diagnostic> {
        if field.depth as usize > self.limits.max_ast_depth {
            return Err(self.bound_error("AST depth", self.limits.max_ast_depth, field.span));
        }
        let id = u32::try_from(self.module.value_fields.len())
            .map_err(|_| self.bound_error("value field arena", u32::MAX as usize, field.span))?;
        self.module.value_fields.push(field);
        let field_id = ValueFieldId(id);
        self.module.value_field_ids.push(field_id);
        Ok(field_id)
    }

    pub(super) fn bound_error(&self, name: &str, limit: usize, span: Span) -> Diagnostic {
        Diagnostic::new(
            "B001",
            format!("{} limit exceeded (limit {})", name, limit),
            span,
        )
    }

    pub(super) fn parse_expression(&mut self) -> Result<ExprId, Diagnostic> {
        expressions::parse_expression(self)
    }

    pub(super) fn parse_type(&mut self) -> Result<TypeId, Diagnostic> {
        super::types::parse_type(self, 1)
    }

    pub(super) fn parse_block(&mut self) -> Result<StatementId, Diagnostic> {
        statements::parse_block(self)
    }
}

pub(super) fn join_span(first: Span, last: Span) -> Span {
    Span::new(first.start_byte, last.end_byte, first.start, last.end)
}

fn bounded_name(name: &str) -> String {
    let mut result = String::new();
    let mut characters = name.chars();
    for character in characters.by_ref().take(32) {
        result.push(character);
    }
    if characters.next().is_some() {
        result.push('…');
    }
    result
}

fn keyword_name(keyword: Keyword) -> &'static str {
    match keyword {
        Keyword::Scalar => "scalar",
        Keyword::Validator => "validator",
        Keyword::Procedure => "procedure",
        Keyword::Fn => "fn",
        Keyword::Let => "let",
        Keyword::Const => "const",
        Keyword::If => "if",
        Keyword::Else => "else",
        Keyword::While => "while",
        Keyword::For => "for",
        Keyword::In => "in",
        Keyword::Return => "return",
        Keyword::Break => "break",
        Keyword::Continue => "continue",
        Keyword::Throw => "throw",
        Keyword::True => "true",
        Keyword::False => "false",
        Keyword::Null => "null",
        Keyword::I32 => "i32",
        Keyword::I64 => "i64",
        Keyword::F64 => "f64",
        Keyword::Bool => "bool",
        Keyword::String => "string",
        Keyword::Bytes => "bytes",
        Keyword::Validation => "Validation",
        Keyword::Batch => "Batch",
        Keyword::Request => "Request",
        Keyword::Response => "Response",
    }
}

fn punctuation_name(value: Punctuation) -> &'static str {
    match value {
        Punctuation::LeftParen => "(",
        Punctuation::RightParen => ")",
        Punctuation::LeftBrace => "{",
        Punctuation::RightBrace => "}",
        Punctuation::LeftBracket => "[",
        Punctuation::RightBracket => "]",
        Punctuation::Comma => ",",
        Punctuation::Colon => ":",
        Punctuation::Semicolon => ";",
        Punctuation::Dot => ".",
        Punctuation::Question => "?",
    }
}

fn operator_name(value: Operator) -> &'static str {
    match value {
        Operator::Assign => "=",
        Operator::Plus => "+",
        Operator::Minus => "-",
        Operator::Star => "*",
        Operator::Slash => "/",
        Operator::Percent => "%",
        Operator::Bang => "!",
        Operator::EqualEqual => "==",
        Operator::BangEqual => "!=",
        Operator::Less => "<",
        Operator::LessEqual => "<=",
        Operator::Greater => ">",
        Operator::GreaterEqual => ">=",
        Operator::AndAnd => "&&",
        Operator::OrOr => "||",
        Operator::Range => "..",
        Operator::Arrow => "->",
    }
}
