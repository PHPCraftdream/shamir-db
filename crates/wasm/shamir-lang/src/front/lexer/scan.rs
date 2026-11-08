use crate::api::{CompileLimits, Diagnostic, Position, Span};
use crate::front::CompileBudget;

use super::line_index::{boundary_at_or_after, boundary_at_or_before, LineIndex};
use super::token::{
    IntegerLiteral, IntegerRadix, IntegerSuffix, Keyword, Operator, Punctuation, Token, TokenKind,
};

/// Scans source text into tokens and appends one EOF token.
pub fn scan(source: &str, limits: &CompileLimits) -> Result<Vec<Token>, Diagnostic> {
    let limits = normalized_limits(limits)?;
    check_source_limit(source, &limits)?;
    let mut budget = CompileBudget::new(&limits).map_err(|error| {
        Diagnostic::new(
            "B001",
            format!("invalid zero compiler limit: {}", error.field),
            Span::source_start(),
        )
    })?;
    scan_with_budget(source, &limits, &mut budget)
}

/// Scans source using a caller-owned budget shared with later pipeline stages.
pub fn scan_with_budget(
    source: &str,
    limits: &CompileLimits,
    budget: &mut CompileBudget,
) -> Result<Vec<Token>, Diagnostic> {
    let limits = normalized_limits(limits)?;
    check_source_limit(source, &limits)?;
    let source_length = u32::try_from(source.len()).map_err(|_| {
        Diagnostic::new(
            "B001",
            "source is too large for byte spans",
            Span::source_start(),
        )
    })?;
    let lines = LineIndex::build(source, budget)?;
    Scanner {
        source,
        limits,
        budget,
        lines,
        source_length,
        offset: 0,
        literal_total: 0,
    }
    .run()
}

fn normalized_limits(limits: &CompileLimits) -> Result<CompileLimits, Diagnostic> {
    limits.clamped().map_err(|error| {
        Diagnostic::new(
            "B001",
            format!("invalid zero compiler limit: {}", error.field),
            Span::source_start(),
        )
    })
}

fn check_source_limit(source: &str, limits: &CompileLimits) -> Result<(), Diagnostic> {
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
    Ok(())
}

struct Scanner<'a> {
    source: &'a str,
    limits: CompileLimits,
    budget: &'a mut CompileBudget,
    lines: LineIndex,
    source_length: u32,
    offset: usize,
    literal_total: usize,
}

impl Scanner<'_> {
    fn run(mut self) -> Result<Vec<Token>, Diagnostic> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia()?;
            let start = self.offset;
            let start_position = self.position_for_token(start)?;
            if start == self.source.len() {
                self.ensure_token_capacity(tokens.len(), start)?;
                self.budget_charge(1, start)?;
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    span: self.span(start, start, start_position, start_position)?,
                });
                return Ok(tokens);
            }
            self.ensure_token_capacity(tokens.len(), start)?;
            let kind = self.scan_token(start)?;
            if let TokenKind::Integer(literal) = &kind {
                if literal.negative_min_candidate
                    && !tokens.last().is_some_and(|token| {
                        matches!(token.kind, TokenKind::Operator(Operator::Minus))
                    })
                {
                    return Err(self.error(
                        "L002",
                        "minimum integer magnitude is valid only after unary minus",
                        start,
                        self.offset,
                    ));
                }
            }
            let end = self.offset;
            let end_position = self.position_for_token(end)?;
            self.budget_charge(1, start)?;
            tokens.push(Token {
                kind,
                span: self.span(start, end, start_position, end_position)?,
            });
        }
    }

    fn scan_token(&mut self, start: usize) -> Result<TokenKind, Diagnostic> {
        self.charge(1, start)?;
        let byte = self.byte_at(start).ok_or_else(|| self.invalid_at(start))?;
        if byte == b'@' {
            return self.scan_annotation(start);
        }
        if byte == b'b' && self.byte_at(start + 1) == Some(b'"') {
            self.offset = start + 1;
            return self.scan_quoted(start, true);
        }
        if byte == b'"' {
            return self.scan_quoted(start, false);
        }
        if byte.is_ascii_digit() {
            return self.scan_number(start);
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            return self.scan_identifier(start);
        }
        if !byte.is_ascii() {
            return Err(self.invalid_at(start));
        }
        self.scan_symbol(start)
    }

    fn skip_trivia(&mut self) -> Result<(), Diagnostic> {
        loop {
            let Some(byte) = self.byte_at(self.offset) else {
                return Ok(());
            };
            if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
                self.charge(1, self.offset)?;
                self.offset += 1;
            } else if byte == b'/' && self.byte_at(self.offset + 1) == Some(b'/') {
                self.charge(2, self.offset)?;
                self.offset += 2;
                while let Some(character) = self.char_at(self.offset) {
                    if matches!(character, '\r' | '\n') {
                        break;
                    }
                    self.charge(character.len_utf8() as u64, self.offset)?;
                    self.offset += character.len_utf8();
                }
            } else if byte == b'/' && self.byte_at(self.offset + 1) == Some(b'*') {
                let comment_start = self.offset;
                self.charge(2, self.offset)?;
                self.offset += 2;
                loop {
                    let Some(character) = self.char_at(self.offset) else {
                        return Err(self.error(
                            "L003",
                            "unterminated block comment",
                            comment_start,
                            self.offset,
                        ));
                    };
                    self.charge(character.len_utf8() as u64, self.offset)?;
                    if character == '*' && self.byte_at(self.offset + 1) == Some(b'/') {
                        self.charge(1, self.offset + 1)?;
                        self.offset += 2;
                        break;
                    }
                    self.offset += character.len_utf8();
                }
            } else {
                return Ok(());
            }
        }
    }

    fn scan_annotation(&mut self, start: usize) -> Result<TokenKind, Diagnostic> {
        const ENTRY: &str = "@entry";
        let Some(text) = self.source.get(start..) else {
            return Err(self.invalid_at(start));
        };
        self.charge(ENTRY.len() as u64, start)?;
        if !text.starts_with(ENTRY) {
            return Err(self.invalid_at(start));
        }
        let end = start + ENTRY.len();
        if self
            .byte_at(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(self.error("L001", "invalid annotation boundary", start, end + 1));
        }
        self.offset = end;
        Ok(TokenKind::AnnotationEntry)
    }

    fn scan_identifier(&mut self, start: usize) -> Result<TokenKind, Diagnostic> {
        let mut end = start;
        while let Some(byte) = self.byte_at(end) {
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                self.charge(1, end)?;
                end += 1;
            } else {
                break;
            }
        }
        let text = self
            .source
            .get(start..end)
            .ok_or_else(|| self.invalid_at(start))?;
        self.charge(text.len() as u64, start)?;
        self.offset = end;
        Ok(match keyword(text) {
            Some(value) => TokenKind::Keyword(value),
            None => TokenKind::Identifier(text.to_owned()),
        })
    }

    fn scan_number(&mut self, start: usize) -> Result<TokenKind, Diagnostic> {
        if self.byte_at(start) == Some(b'0') && matches!(self.byte_at(start + 1), Some(b'x' | b'X'))
        {
            return self.scan_hex(start);
        }
        self.scan_decimal(start)
    }

    fn scan_hex(&mut self, start: usize) -> Result<TokenKind, Diagnostic> {
        let mut end = start + 2;
        self.check_numeric_spelling(start, end)?;
        self.charge(1, start + 1)?;
        let digits_start = end;
        while self
            .byte_at(end)
            .is_some_and(|byte| byte.is_ascii_hexdigit())
        {
            self.check_numeric_spelling(start, end + 1)?;
            self.charge(1, end)?;
            end += 1;
        }
        if end == digits_start {
            return Err(self.error(
                "L002",
                "hex integer requires at least one digit",
                start,
                end,
            ));
        }
        let (suffix, suffix_end) = self.integer_suffix(end, start)?;
        end = suffix_end;
        self.reject_numeric_tail(end, start)?;
        self.charge_numeric_literal(start, end)?;
        let digits_end = end - suffix_length(suffix);
        let text = self
            .source
            .get(digits_start..digits_end)
            .ok_or_else(|| self.invalid_at(start))?;
        self.charge(text.len() as u64, start)?;
        let magnitude = u64::from_str_radix(text, 16)
            .map_err(|_| self.error("L002", "integer literal overflows its type", start, end))?;
        let literal =
            self.checked_integer(magnitude, suffix, IntegerRadix::Hexadecimal, start, end)?;
        self.offset = end;
        Ok(TokenKind::Integer(literal))
    }

    fn scan_decimal(&mut self, start: usize) -> Result<TokenKind, Diagnostic> {
        let mut end = start;
        while self.byte_at(end).is_some_and(|byte| byte.is_ascii_digit()) {
            self.check_numeric_spelling(start, end + 1)?;
            self.charge(1, end)?;
            end += 1;
        }
        let integer_end = end;
        let has_fraction = self.byte_at(end) == Some(b'.')
            && self
                .byte_at(end + 1)
                .is_some_and(|byte| byte.is_ascii_digit());
        if has_fraction {
            self.check_numeric_spelling(start, end + 1)?;
            self.charge(1, end)?;
            end += 1;
            while self.byte_at(end).is_some_and(|byte| byte.is_ascii_digit()) {
                self.check_numeric_spelling(start, end + 1)?;
                self.charge(1, end)?;
                end += 1;
            }
        }
        let mut has_exponent = false;
        if matches!(self.byte_at(end), Some(b'e' | b'E')) {
            has_exponent = true;
            let exponent_start = end;
            self.check_numeric_spelling(start, end + 1)?;
            self.charge(1, end)?;
            end += 1;
            if matches!(self.byte_at(end), Some(b'+' | b'-')) {
                self.check_numeric_spelling(start, end + 1)?;
                self.charge(1, end)?;
                end += 1;
            }
            let digits_start = end;
            while self.byte_at(end).is_some_and(|byte| byte.is_ascii_digit()) {
                self.check_numeric_spelling(start, end + 1)?;
                self.charge(1, end)?;
                end += 1;
            }
            if end == digits_start {
                return Err(self.error(
                    "L002",
                    "float exponent requires digits",
                    exponent_start,
                    end,
                ));
            }
        }
        if integer_end - start > 1 && self.byte_at(start) == Some(b'0') {
            return Err(self.error(
                "L002",
                "leading zero in numeric literal",
                start,
                integer_end,
            ));
        }
        if has_fraction || has_exponent {
            let has_suffix = self.matches_at(end, "f64");
            if has_suffix {
                self.check_numeric_spelling(start, end + 3)?;
                self.charge(3, end)?;
                end += 3;
            }
            self.reject_numeric_tail(end, start)?;
            self.charge_numeric_literal(start, end)?;
            let float_end = if has_suffix { end - 3 } else { end };
            let text = self
                .source
                .get(start..float_end)
                .ok_or_else(|| self.invalid_at(start))?;
            self.charge(text.len() as u64, start)?;
            let value = text
                .parse::<f64>()
                .map_err(|_| self.error("L002", "malformed float literal", start, end))?;
            if !value.is_finite() {
                return Err(self.error("L002", "float literal must be finite", start, end));
            }
            self.offset = end;
            return Ok(TokenKind::Float(value));
        }
        let (suffix, suffix_end) = self.integer_suffix(end, start)?;
        end = suffix_end;
        self.reject_numeric_tail(end, start)?;
        self.charge_numeric_literal(start, end)?;
        let digits_end = end - suffix_length(suffix);
        let text = self
            .source
            .get(start..digits_end)
            .ok_or_else(|| self.invalid_at(start))?;
        self.charge(text.len() as u64, start)?;
        let magnitude = text
            .parse::<u64>()
            .map_err(|_| self.error("L002", "integer literal overflows its type", start, end))?;
        let literal = self.checked_integer(magnitude, suffix, IntegerRadix::Decimal, start, end)?;
        self.offset = end;
        Ok(TokenKind::Integer(literal))
    }

    fn integer_suffix(
        &mut self,
        offset: usize,
        literal_start: usize,
    ) -> Result<(Option<IntegerSuffix>, usize), Diagnostic> {
        if self.matches_at(offset, "i32") {
            self.check_numeric_spelling(literal_start, offset + 3)?;
            self.charge(3, offset)?;
            return Ok((Some(IntegerSuffix::I32), offset + 3));
        }
        if self.matches_at(offset, "i64") {
            self.check_numeric_spelling(literal_start, offset + 3)?;
            self.charge(3, offset)?;
            return Ok((Some(IntegerSuffix::I64), offset + 3));
        }
        if self.byte_at(offset) == Some(b'i') {
            return Err(self.error("L002", "invalid integer suffix", offset, offset + 1));
        }
        Ok((None, offset))
    }

    fn checked_integer(
        &self,
        magnitude: u64,
        suffix: Option<IntegerSuffix>,
        radix: IntegerRadix,
        start: usize,
        end: usize,
    ) -> Result<IntegerLiteral, Diagnostic> {
        let (maximum, signed_min_magnitude) = match suffix {
            Some(IntegerSuffix::I32) => (i32::MAX as u64, i32::MAX as u64 + 1),
            Some(IntegerSuffix::I64) | None => (i64::MAX as u64, i64::MAX as u64 + 1),
        };
        if magnitude > signed_min_magnitude {
            return Err(self.error("L002", "integer literal overflows its type", start, end));
        }
        Ok(IntegerLiteral {
            magnitude,
            suffix,
            radix,
            negative_min_candidate: magnitude == maximum + 1,
        })
    }

    fn reject_numeric_tail(&self, end: usize, start: usize) -> Result<(), Diagnostic> {
        if self
            .byte_at(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(self.error("L002", "invalid numeric suffix", start, end + 1));
        }
        Ok(())
    }

    fn scan_quoted(
        &mut self,
        token_start: usize,
        bytes_literal: bool,
    ) -> Result<TokenKind, Diagnostic> {
        self.offset += 1;
        let mut decoded_string = String::new();
        let mut decoded_bytes = Vec::new();
        let mut literal_bytes = 0usize;
        loop {
            let Some(character) = self.char_at(self.offset) else {
                return Err(self.error(
                    "L003",
                    if bytes_literal {
                        "unterminated byte literal"
                    } else {
                        "unterminated string"
                    },
                    token_start,
                    self.offset,
                ));
            };
            self.charge(character.len_utf8() as u64, self.offset)?;
            if character == '"' {
                self.offset += 1;
                break;
            }
            if matches!(character, '\r' | '\n') || is_forbidden_control(character) {
                return Err(self.error(
                    "L002",
                    "raw control character in literal",
                    self.offset,
                    self.offset + character.len_utf8(),
                ));
            }
            if character == '\\' {
                self.offset += 1;
                let decoded = if bytes_literal {
                    Decoded::Byte(self.decode_byte_escape(token_start)?)
                } else {
                    Decoded::Character(self.decode_string_escape(token_start)?)
                };
                let growth = match decoded {
                    Decoded::Character(character) => character.len_utf8(),
                    Decoded::Byte(_) => 1,
                };
                self.charge_literal_growth(growth, literal_bytes, token_start)?;
                literal_bytes += growth;
                match decoded {
                    Decoded::Character(character) => decoded_string.push(character),
                    Decoded::Byte(byte) => decoded_bytes.push(byte),
                }
                continue;
            }
            let growth = character.len_utf8();
            if bytes_literal && growth != 1 {
                return Err(self.error(
                    "L002",
                    "unescaped byte literal character must encode to one byte",
                    self.offset,
                    self.offset + growth,
                ));
            }
            self.charge_literal_growth(
                if bytes_literal { 1 } else { growth },
                literal_bytes,
                token_start,
            )?;
            literal_bytes += if bytes_literal { 1 } else { growth };
            if bytes_literal {
                let byte = u8::try_from(u32::from(character)).map_err(|_| {
                    self.error(
                        "L002",
                        "invalid byte literal character",
                        self.offset,
                        self.offset + growth,
                    )
                })?;
                decoded_bytes.push(byte);
            } else {
                decoded_string.push(character);
            }
            self.offset += growth;
        }
        if bytes_literal {
            Ok(TokenKind::Bytes(decoded_bytes))
        } else {
            Ok(TokenKind::String(decoded_string))
        }
    }

    fn decode_string_escape(&mut self, token_start: usize) -> Result<char, Diagnostic> {
        let Some(escaped) = self.char_at(self.offset) else {
            return Err(self.error(
                "L003",
                "unterminated string escape",
                token_start,
                self.offset,
            ));
        };
        self.charge(escaped.len_utf8() as u64, self.offset)?;
        self.offset += escaped.len_utf8();
        match escaped {
            '"' => Ok('"'),
            '\\' => Ok('\\'),
            '/' => Ok('/'),
            'b' => Ok('\u{0008}'),
            'f' => Ok('\u{000c}'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'u' if self.byte_at(self.offset) == Some(b'{') => self.decode_unicode_escape(),
            _ => Err(self.error(
                "L002",
                "invalid string escape",
                self.offset - escaped.len_utf8(),
                self.offset,
            )),
        }
    }

    fn decode_unicode_escape(&mut self) -> Result<char, Diagnostic> {
        let escape_start = self.offset.saturating_sub(1);
        self.charge(1, self.offset)?;
        self.offset += 1;
        let mut value = 0u32;
        let mut digit_count = 0usize;
        while let Some(byte) = self.byte_at(self.offset) {
            if byte == b'}' {
                break;
            }
            self.charge(1, self.offset)?;
            let Some(digit) = hex_value(byte) else {
                return Err(self.error(
                    "L002",
                    "invalid Unicode escape",
                    escape_start,
                    self.offset + 1,
                ));
            };
            digit_count += 1;
            if digit_count > 6 {
                return Err(self.error(
                    "L002",
                    "Unicode escape has more than six digits",
                    escape_start,
                    self.offset + 1,
                ));
            }
            value = value * 16 + u32::from(digit);
            self.offset += 1;
        }
        if digit_count == 0 || self.byte_at(self.offset) != Some(b'}') {
            if self.byte_at(self.offset).is_none() {
                return Err(self.error(
                    "L003",
                    "unterminated Unicode escape",
                    escape_start,
                    self.offset,
                ));
            }
            return Err(self.error(
                "L002",
                "Unicode escape requires one to six hex digits and `}`",
                escape_start,
                self.offset,
            ));
        }
        self.charge(1, self.offset)?;
        self.offset += 1;
        char::from_u32(value).ok_or_else(|| {
            self.error(
                "L002",
                "Unicode escape is not a scalar value",
                escape_start,
                self.offset,
            )
        })
    }

    fn decode_byte_escape(&mut self, token_start: usize) -> Result<u8, Diagnostic> {
        let Some(escaped) = self.char_at(self.offset) else {
            return Err(self.error("L003", "unterminated byte escape", token_start, self.offset));
        };
        self.charge(escaped.len_utf8() as u64, self.offset)?;
        self.offset += escaped.len_utf8();
        match escaped {
            '"' => Ok(b'"'),
            '\\' => Ok(b'\\'),
            'n' => Ok(b'\n'),
            'r' => Ok(b'\r'),
            't' => Ok(b'\t'),
            '0' => Ok(0),
            'x' => {
                let high = self.byte_at(self.offset).and_then(hex_value);
                let low = self.byte_at(self.offset + 1).and_then(hex_value);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        self.charge(2, self.offset)?;
                        self.offset += 2;
                        Ok(high * 16 + low)
                    }
                    _ if self.byte_at(self.offset).is_none()
                        || (self.byte_at(self.offset) != Some(b'"')
                            && self.byte_at(self.offset + 1).is_none()) =>
                    {
                        Err(self.error(
                            "L003",
                            "unterminated byte escape",
                            token_start,
                            self.offset,
                        ))
                    }
                    _ => Err(self.error(
                        "L002",
                        "byte `\\x` escape requires two hex digits",
                        self.offset.saturating_sub(2),
                        self.offset.saturating_add(2),
                    )),
                }
            }
            _ => Err(self.error(
                "L002",
                "invalid byte escape",
                self.offset - escaped.len_utf8(),
                self.offset,
            )),
        }
    }

    fn charge_numeric_literal(&mut self, start: usize, end: usize) -> Result<(), Diagnostic> {
        let literal_bytes = end.saturating_sub(start);
        let next_total = self
            .literal_total
            .checked_add(literal_bytes)
            .ok_or_else(|| self.error("B001", "total literal byte limit exceeded", start, end))?;
        if next_total > self.limits.max_total_literal_bytes {
            return Err(self.error(
                "B001",
                format!(
                    "total literal byte limit exceeded (limit {})",
                    self.limits.max_total_literal_bytes
                ),
                start,
                end,
            ));
        }
        self.charge(literal_bytes as u64, start)?;
        self.literal_total = next_total;
        Ok(())
    }

    fn check_numeric_spelling(&self, start: usize, end: usize) -> Result<(), Diagnostic> {
        let literal_bytes = end.saturating_sub(start);
        let next_total = self
            .literal_total
            .checked_add(literal_bytes)
            .ok_or_else(|| self.error("B001", "total literal byte limit exceeded", start, end))?;
        if literal_bytes > self.limits.max_single_literal_bytes {
            return Err(self.error(
                "B001",
                format!(
                    "single literal byte limit exceeded (limit {})",
                    self.limits.max_single_literal_bytes
                ),
                start,
                end,
            ));
        }
        if next_total > self.limits.max_total_literal_bytes {
            return Err(self.error(
                "B001",
                format!(
                    "total literal byte limit exceeded (limit {})",
                    self.limits.max_total_literal_bytes
                ),
                start,
                end,
            ));
        }
        Ok(())
    }

    fn charge_literal_growth(
        &mut self,
        growth: usize,
        current_single: usize,
        start: usize,
    ) -> Result<(), Diagnostic> {
        let next_single = current_single.checked_add(growth).ok_or_else(|| {
            self.error(
                "B001",
                "single literal byte limit exceeded",
                start,
                self.offset,
            )
        })?;
        if next_single > self.limits.max_single_literal_bytes {
            return Err(self.error(
                "B001",
                format!(
                    "single literal byte limit exceeded (limit {})",
                    self.limits.max_single_literal_bytes
                ),
                start,
                self.offset,
            ));
        }
        let total = self.literal_total.checked_add(growth).ok_or_else(|| {
            self.error(
                "B001",
                "total literal byte limit exceeded",
                start,
                self.offset,
            )
        })?;
        if total > self.limits.max_total_literal_bytes {
            return Err(self.error(
                "B001",
                format!(
                    "total literal byte limit exceeded (limit {})",
                    self.limits.max_total_literal_bytes
                ),
                start,
                self.offset,
            ));
        }
        self.charge(growth as u64, self.offset)?;
        self.literal_total = total;
        Ok(())
    }

    fn scan_symbol(&mut self, start: usize) -> Result<TokenKind, Diagnostic> {
        let pair = self.byte_at(start + 1);
        let (kind, width) = match (self.byte_at(start), pair) {
            (Some(b'='), Some(b'=')) => (TokenKind::Operator(Operator::EqualEqual), 2),
            (Some(b'!'), Some(b'=')) => (TokenKind::Operator(Operator::BangEqual), 2),
            (Some(b'<'), Some(b'=')) => (TokenKind::Operator(Operator::LessEqual), 2),
            (Some(b'>'), Some(b'=')) => (TokenKind::Operator(Operator::GreaterEqual), 2),
            (Some(b'&'), Some(b'&')) => (TokenKind::Operator(Operator::AndAnd), 2),
            (Some(b'|'), Some(b'|')) => (TokenKind::Operator(Operator::OrOr), 2),
            (Some(b'.'), Some(b'.')) => (TokenKind::Operator(Operator::Range), 2),
            (Some(b'-'), Some(b'>')) => (TokenKind::Operator(Operator::Arrow), 2),
            _ => match self.byte_at(start) {
                Some(b'=') => (TokenKind::Operator(Operator::Assign), 1),
                Some(b'+') => (TokenKind::Operator(Operator::Plus), 1),
                Some(b'-') => (TokenKind::Operator(Operator::Minus), 1),
                Some(b'*') => (TokenKind::Operator(Operator::Star), 1),
                Some(b'/') => (TokenKind::Operator(Operator::Slash), 1),
                Some(b'%') => (TokenKind::Operator(Operator::Percent), 1),
                Some(b'!') => (TokenKind::Operator(Operator::Bang), 1),
                Some(b'<') => (TokenKind::Operator(Operator::Less), 1),
                Some(b'>') => (TokenKind::Operator(Operator::Greater), 1),
                Some(b'(') => (TokenKind::Punctuation(Punctuation::LeftParen), 1),
                Some(b')') => (TokenKind::Punctuation(Punctuation::RightParen), 1),
                Some(b'{') => (TokenKind::Punctuation(Punctuation::LeftBrace), 1),
                Some(b'}') => (TokenKind::Punctuation(Punctuation::RightBrace), 1),
                Some(b'[') => (TokenKind::Punctuation(Punctuation::LeftBracket), 1),
                Some(b']') => (TokenKind::Punctuation(Punctuation::RightBracket), 1),
                Some(b',') => (TokenKind::Punctuation(Punctuation::Comma), 1),
                Some(b':') => (TokenKind::Punctuation(Punctuation::Colon), 1),
                Some(b';') => (TokenKind::Punctuation(Punctuation::Semicolon), 1),
                Some(b'.') => (TokenKind::Punctuation(Punctuation::Dot), 1),
                Some(b'?') => (TokenKind::Punctuation(Punctuation::Question), 1),
                _ => return Err(self.invalid_at(start)),
            },
        };
        self.charge(width as u64, start)?;
        self.offset = start + width;
        Ok(kind)
    }

    fn ensure_token_capacity(&self, count: usize, offset: usize) -> Result<(), Diagnostic> {
        if count >= self.limits.max_tokens {
            return Err(self.error(
                "B001",
                format!("token limit exceeded (limit {})", self.limits.max_tokens),
                offset,
                offset,
            ));
        }
        Ok(())
    }

    fn charge(&mut self, amount: u64, offset: usize) -> Result<(), Diagnostic> {
        self.budget_charge(amount, offset)
    }

    fn budget_charge(&mut self, amount: u64, offset: usize) -> Result<(), Diagnostic> {
        if self.budget.charge(amount, Span::source_start()).is_err() {
            return Err(self.error("B001", "compile fuel exceeded", offset, offset));
        }
        Ok(())
    }

    fn invalid_at(&self, offset: usize) -> Diagnostic {
        let end = self
            .char_at(offset)
            .map_or(offset, |character| offset + character.len_utf8());
        self.error("L001", "invalid character", offset, end)
    }

    fn error(
        &self,
        code: &'static str,
        message: impl Into<String>,
        start: usize,
        end: usize,
    ) -> Diagnostic {
        let start = boundary_at_or_before(self.source, start.min(self.source.len()));
        let end = boundary_at_or_after(self.source, end.min(self.source.len())).max(start);
        let start_byte = match u32::try_from(start) {
            Ok(value) => value,
            Err(_) => self.source_length,
        };
        let end_byte = match u32::try_from(end) {
            Ok(value) => value,
            Err(_) => self.source_length,
        };
        Diagnostic::new(
            code,
            message,
            Span::new(
                start_byte,
                end_byte,
                self.position(start),
                self.position(end),
            ),
        )
    }

    fn span(
        &self,
        start: usize,
        end: usize,
        start_pos: Position,
        end_pos: Position,
    ) -> Result<Span, Diagnostic> {
        let start_byte = u32::try_from(start).map_err(|_| {
            Diagnostic::new(
                "B001",
                "source is too large for byte spans",
                Span::source_start(),
            )
        })?;
        let end_byte = u32::try_from(end).map_err(|_| {
            Diagnostic::new(
                "B001",
                "source is too large for byte spans",
                Span::source_start(),
            )
        })?;
        Ok(Span::new(start_byte, end_byte, start_pos, end_pos))
    }

    fn position_for_token(&mut self, offset: usize) -> Result<Position, Diagnostic> {
        let source = self.source;
        self.lines.position_at_metered(source, offset, self.budget)
    }

    fn position(&self, offset: usize) -> Position {
        self.lines.position_at(self.source, offset)
    }

    fn byte_at(&self, offset: usize) -> Option<u8> {
        self.source.as_bytes().get(offset).copied()
    }

    fn char_at(&self, offset: usize) -> Option<char> {
        self.source.get(offset..)?.chars().next()
    }

    fn matches_at(&self, offset: usize, text: &str) -> bool {
        self.source
            .get(offset..)
            .is_some_and(|remaining| remaining.starts_with(text))
    }
}

#[derive(Clone, Copy)]
enum Decoded {
    Character(char),
    Byte(u8),
}

fn keyword(text: &str) -> Option<Keyword> {
    Some(match text {
        "scalar" => Keyword::Scalar,
        "validator" => Keyword::Validator,
        "procedure" => Keyword::Procedure,
        "fn" => Keyword::Fn,
        "let" => Keyword::Let,
        "const" => Keyword::Const,
        "if" => Keyword::If,
        "else" => Keyword::Else,
        "while" => Keyword::While,
        "for" => Keyword::For,
        "in" => Keyword::In,
        "return" => Keyword::Return,
        "break" => Keyword::Break,
        "continue" => Keyword::Continue,
        "throw" => Keyword::Throw,
        "true" => Keyword::True,
        "false" => Keyword::False,
        "null" => Keyword::Null,
        "i32" => Keyword::I32,
        "i64" => Keyword::I64,
        "f64" => Keyword::F64,
        "bool" => Keyword::Bool,
        "string" => Keyword::String,
        "bytes" => Keyword::Bytes,
        "Validation" => Keyword::Validation,
        "Batch" => Keyword::Batch,
        "Request" => Keyword::Request,
        "Response" => Keyword::Response,
        _ => return None,
    })
}

fn suffix_length(suffix: Option<IntegerSuffix>) -> usize {
    suffix.map_or(0, |_| 3)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_forbidden_control(character: char) -> bool {
    character <= '\u{001f}' || character == '\u{007f}'
}
