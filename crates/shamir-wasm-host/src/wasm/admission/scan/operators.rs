use crate::wasm::admission::types::{AdmissionError, AdmissionLimits};
use wasmparser::{FunctionBody, Operator};

fn wide(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Text name of a reference-type operator we do not admit, if it is one.
/// `ref.func` stays legal in constant expressions (never scanned here) and
/// `call_indirect` stays legal everywhere.
fn forbidden_name(op: &Operator<'_>) -> Option<&'static str> {
    match op {
        Operator::RefNull { .. } => Some("ref.null"),
        Operator::RefIsNull => Some("ref.is_null"),
        Operator::RefFunc { .. } => Some("ref.func"),
        Operator::TableGet { .. } => Some("table.get"),
        Operator::TableSet { .. } => Some("table.set"),
        Operator::TableGrow { .. } => Some("table.grow"),
        Operator::TableSize { .. } => Some("table.size"),
        Operator::TableFill { .. } => Some("table.fill"),
        Operator::TableCopy { .. } => Some("table.copy"),
        Operator::TableInit { .. } => Some("table.init"),
        Operator::ElemDrop { .. } => Some("elem.drop"),
        _ => None,
    }
}

/// Scans one function body; returns `(operator count, deepest block nesting)`.
///
/// Forbidden operators are rejected under every policy; the numeric caps
/// only when `limits` is given.
pub(super) fn scan_operators(
    body: &FunctionBody<'_>,
    function_index: u32,
    limits: Option<&AdmissionLimits>,
) -> Result<(u64, u64), AdmissionError> {
    let mut reader = body
        .get_operators_reader()
        .map_err(|e| AdmissionError::Parse(e.to_string()))?;
    let mut operators: u64 = 0;
    let mut depth: u64 = 0;
    let mut max_depth: u64 = 0;
    while !reader.eof() {
        let op = reader
            .read()
            .map_err(|e| AdmissionError::Parse(e.to_string()))?;
        operators += 1;
        if let Some(limits) = limits {
            let limit = wide(limits.max_operators_per_function);
            if operators > limit {
                return Err(AdmissionError::TooManyOperators {
                    function_index,
                    limit,
                    observed: operators,
                });
            }
        }
        if let Some(operator) = forbidden_name(&op) {
            return Err(AdmissionError::ForbiddenOperator {
                function_index,
                operator,
            });
        }
        match op {
            Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } => {
                depth += 1;
                max_depth = max_depth.max(depth);
                if let Some(limits) = limits {
                    let limit = wide(limits.max_block_depth);
                    if depth > limit {
                        return Err(AdmissionError::BlockDepthExceeded {
                            function_index,
                            limit,
                            observed: depth,
                        });
                    }
                }
            }
            Operator::End => depth = depth.saturating_sub(1),
            Operator::BrTable { targets } => {
                if let Some(limits) = limits {
                    let limit = wide(limits.max_br_table_targets);
                    let observed = u64::from(targets.len());
                    if observed > limit {
                        return Err(AdmissionError::BrTableTooLarge {
                            function_index,
                            limit,
                            observed,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    Ok((operators, max_depth))
}
