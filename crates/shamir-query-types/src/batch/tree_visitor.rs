//! Iterative, insertion-order preorder traversal of the static operation tree.

use shamir_collections::TMap;

use super::{BatchOp, QueryEntry};

/// One borrowed entry in a [`BatchTreeVisitor`] walk.
#[derive(Clone, Copy)]
pub struct BatchTreeEntry<'a> {
    /// The static entry, including execution/return gates (not evaluated).
    pub entry: &'a QueryEntry,
    /// Stable local map key. Aliases may repeat in different bodies; this is
    /// not a globally unique path. The borrowed entry identifies the slot.
    pub alias: &'a str,
    /// Depth of the batch containing this entry; root entries have depth 0.
    pub depth: usize,
    /// Saturating product of the lengths of the literal `over` lists of the enclosing
    /// ForEach loops (a non-literal `over` counts as 1).
    pub multiplier: u64,
}

impl BatchTreeEntry<'_> {
    /// Depth of a container's body, including an empty body; None for leaves.
    pub fn body_depth(&self) -> Option<usize> {
        BatchTreeVisitor::body_queries(&self.entry.op).map(|_| self.depth + 1)
    }
}

/// Visits every entry once, containers before children, in map insertion order.
///
/// Both `Batch` and `ForEach` add one body level. Empty bodies are represented
/// by their container entry's `body_depth()`. The complete static loop template
/// is visited once regardless of `over`, `when`, or `return_result`. No limits
/// are imposed and no recursive calls or owned DTO clones are used.
/// The worklist holds borrowed entries only, so early drop is also iterative.
pub struct BatchTreeVisitor<'a> {
    pending: Vec<BatchTreeEntry<'a>>,
}

impl<'a> BatchTreeVisitor<'a> {
    pub fn new(queries: &'a TMap<String, QueryEntry>) -> Self {
        let mut visitor = Self {
            pending: Vec::new(),
        };
        visitor.push_queries(queries, 0, 1);
        visitor
    }

    pub(crate) fn body_queries(op: &BatchOp) -> Option<&TMap<String, QueryEntry>> {
        match op {
            BatchOp::Batch(sub) => Some(&sub.batch.queries),
            BatchOp::ForEach(fe) => Some(&fe.batch.queries),
            _ => None,
        }
    }

    fn push_queries(
        &mut self,
        queries: &'a TMap<String, QueryEntry>,
        depth: usize,
        multiplier: u64,
    ) {
        self.pending
            .extend(queries.iter().rev().map(|(alias, entry)| BatchTreeEntry {
                entry,
                alias: alias.as_str(),
                depth,
                multiplier,
            }));
    }
}

impl<'a> Iterator for BatchTreeVisitor<'a> {
    type Item = BatchTreeEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let visit = self.pending.pop()?;
        if let Some(queries) = Self::body_queries(&visit.entry.op) {
            let multiplier = match &visit.entry.op {
                BatchOp::ForEach(fe) => match &fe.over {
                    crate::filter::FilterValue::Array(items) => {
                        visit.multiplier.saturating_mul(items.len() as u64)
                    }
                    _ => visit.multiplier,
                },
                _ => visit.multiplier,
            };
            self.push_queries(queries, visit.depth + 1, multiplier);
        }
        Some(visit)
    }
}
