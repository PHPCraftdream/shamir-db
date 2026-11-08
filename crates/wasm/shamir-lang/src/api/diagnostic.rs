use std::fmt::{self, Display, Formatter};

use super::Span;

/// Maximum UTF-8 byte length of a diagnostic message.
pub const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 1024;

/// A stable, source-located compiler diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Stable diagnostic code.
    pub code: &'static str,
    /// Bounded human-readable message.
    pub message: String,
    /// Primary source location.
    pub primary: Span,
    /// Optional related location, rendered after the primary location.
    pub related: Option<RelatedSpan>,
}

/// A secondary source location associated with a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelatedSpan {
    /// Related source range.
    pub span: Span,
    /// Bounded explanation of the related location.
    pub message: String,
}

impl Diagnostic {
    /// Creates a diagnostic, truncating the message only at a UTF-8 boundary.
    pub fn new(code: &'static str, message: impl Into<String>, primary: Span) -> Self {
        Self {
            code,
            message: bounded_message(message.into()),
            primary,
            related: None,
        }
    }

    /// Adds one bounded related location.
    pub fn with_related(mut self, span: Span, message: impl Into<String>) -> Self {
        self.related = Some(RelatedSpan {
            span,
            message: bounded_message(message.into()),
        });
        self
    }
}

fn bounded_message(mut message: String) -> String {
    if message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES {
        return message;
    }
    let mut boundary = MAX_DIAGNOSTIC_MESSAGE_BYTES;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message
}

impl Display for Diagnostic {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}: {}",
            self.code, self.primary.start.line, self.primary.start.column, self.message
        )?;
        if let Some(related) = &self.related {
            write!(
                formatter,
                "\n  related {}:{}: {}",
                related.span.start.line, related.span.start.column, related.message
            )?;
        }
        Ok(())
    }
}
