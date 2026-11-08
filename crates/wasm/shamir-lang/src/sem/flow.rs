use crate::front::{Module, StatementId, StatementKind};

/// Conservative recursion ceiling for flow walks.
pub const MAX_FLOW_DEPTH: u32 = 128;

/// True iff every reachable path through `stmt` ends in `return` or `throw`.
/// Loops may run zero times and break/continue exit via the loop, so neither
/// establishes return coverage.
pub fn always_terminates(module: &Module, stmt: StatementId, depth: u32) -> bool {
    if depth >= MAX_FLOW_DEPTH {
        return false;
    }
    let kind = match statement(module, stmt) {
        Some(kind) => kind,
        None => return false,
    };
    match kind {
        StatementKind::Block(stmts) => {
            for inner in stmts {
                if always_terminates(module, *inner, depth.saturating_add(1)) {
                    return true;
                }
            }
            false
        }
        StatementKind::If {
            then_block,
            else_branch,
            ..
        } => match else_branch {
            Some(else_block) => {
                always_terminates(module, *then_block, depth.saturating_add(1))
                    && always_terminates(module, *else_block, depth.saturating_add(1))
            }
            None => false,
        },
        StatementKind::Return(_) => true,
        StatementKind::Throw(_) => true,
        StatementKind::Let { .. }
        | StatementKind::Assign { .. }
        | StatementKind::While { .. }
        | StatementKind::For { .. }
        | StatementKind::Break
        | StatementKind::Continue
        | StatementKind::Expression(_) => false,
    }
}

/// Looks up one statement kind without unchecked arena access.
fn statement(module: &Module, id: StatementId) -> Option<&StatementKind> {
    module.statement(id).map(|stmt| &stmt.kind)
}
