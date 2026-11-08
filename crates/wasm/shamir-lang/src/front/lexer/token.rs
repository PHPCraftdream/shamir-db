/// Reserved language keywords.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Keyword {
    Scalar,
    Validator,
    Procedure,
    Fn,
    Let,
    Const,
    If,
    Else,
    While,
    For,
    In,
    Return,
    Break,
    Continue,
    Throw,
    True,
    False,
    Null,
    I32,
    I64,
    F64,
    Bool,
    String,
    Bytes,
    Validation,
    Batch,
    Request,
    Response,
}

/// Numeric suffix selecting an integer type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntegerSuffix {
    I32,
    I64,
}

/// Base used to spell an integer literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntegerRadix {
    Decimal,
    Hexadecimal,
}

/// Integer magnitude and its source-level typing metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IntegerLiteral {
    /// Unsigned magnitude; the parser must accept a candidate only as the
    /// direct operand of unary minus, never as a binary subtraction operand.
    pub magnitude: u64,
    /// Explicit integer suffix, if present.
    pub suffix: Option<IntegerSuffix>,
    /// Literal radix.
    pub radix: IntegerRadix,
    /// Exactly one above the signed maximum; the parser accepts it only as
    /// the direct operand of unary minus.
    pub negative_min_candidate: bool,
}

/// Operators recognized by the language lexer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operator {
    Assign,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    EqualEqual,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    AndAnd,
    OrOr,
    Range,
    Arrow,
}

/// Delimiters and punctuation recognized by the language lexer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Punctuation {
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Colon,
    Semicolon,
    Dot,
    Question,
}

/// Token payloads emitted by the scanner.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// ASCII identifier (keywords are represented separately).
    Identifier(String),
    /// Reserved keyword.
    Keyword(Keyword),
    /// The complete, boundary-checked `@entry` annotation.
    AnnotationEntry,
    /// Integer literal retaining its magnitude and suffix metadata.
    Integer(IntegerLiteral),
    /// Finite `f64` literal.
    Float(f64),
    /// Decoded UTF-8 string literal.
    String(String),
    /// Decoded bytes literal.
    Bytes(Vec<u8>),
    /// Operator.
    Operator(Operator),
    /// Delimiter or punctuation.
    Punctuation(Punctuation),
    /// End of input.
    Eof,
}

/// One token and its exact half-open source span.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    /// Token kind and decoded payload.
    pub kind: TokenKind,
    /// UTF-8 byte and Unicode-scalar source span.
    pub span: crate::api::Span,
}
