/// A one-based source position measured in Unicode scalar values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Position {
    /// One-based source line.
    pub line: u32,
    /// One-based Unicode-scalar column.
    pub column: u32,
}

/// A half-open UTF-8 byte range with its corresponding source positions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    /// Inclusive byte offset.
    pub start_byte: u32,
    /// Exclusive byte offset.
    pub end_byte: u32,
    /// Position at `start_byte`.
    pub start: Position,
    /// Position at `end_byte`.
    pub end: Position,
}

impl Span {
    /// Creates a zero-width span at the beginning of a source.
    pub const fn source_start() -> Self {
        Self {
            start_byte: 0,
            end_byte: 0,
            start: Position { line: 1, column: 1 },
            end: Position { line: 1, column: 1 },
        }
    }

    /// Creates a span from already checked byte offsets and positions.
    pub const fn new(start_byte: u32, end_byte: u32, start: Position, end: Position) -> Self {
        Self {
            start_byte,
            end_byte,
            start,
            end,
        }
    }
}
