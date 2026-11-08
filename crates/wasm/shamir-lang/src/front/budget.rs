use crate::api::{CompileLimits, Diagnostic, Span};

/// Shared monotonic compiler-work budget for front-end stages.
///
/// Each stage charges before scanning, copying, appending or traversing work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileBudget {
    limit: u64,
    spent: u64,
}

impl CompileBudget {
    /// Creates a budget from normalized compiler limits.
    pub fn new(limits: &CompileLimits) -> Result<Self, crate::api::LimitError> {
        let limits = limits.clamped()?;
        Ok(Self {
            limit: limits.compile_fuel,
            spent: 0,
        })
    }

    /// Charges work before it is performed, returning `B001` at the charged span.
    pub fn charge(&mut self, amount: u64, span: Span) -> Result<(), Diagnostic> {
        let Some(next) = self.spent.checked_add(amount) else {
            return Err(self.exhausted(span));
        };
        if next > self.limit {
            return Err(self.exhausted(span));
        }
        self.spent = next;
        Ok(())
    }

    /// Returns consumed fuel.
    pub const fn spent(&self) -> u64 {
        self.spent
    }

    /// Returns the normalized fuel ceiling.
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    fn exhausted(&self, span: Span) -> Diagnostic {
        Diagnostic::new(
            "B001",
            format!("compile fuel exceeded (limit {})", self.limit),
            span,
        )
    }
}
