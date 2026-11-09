use std::collections::VecDeque;

use crate::front::{EntryKind, ExprId, FunctionId};
use crate::sem::effects::{BuiltinCategory, EffectSet};

use super::entry::{capability_of, expr_span, report, report_related, Checker};

/// Builds a deterministic topological call order with callees before callers,
/// reporting recursion.
pub(super) fn build_call_order(c: &mut Checker) -> Vec<FunctionId> {
    let count = c.module.functions.len();
    let cyclic = find_cyclic(c);
    let adjacency = remaining_adjacency(c, &cyclic);
    // Out-degree Kahn over the acyclic remainder: callees are emitted first.
    let mut reverse: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (caller, edges) in adjacency.iter().enumerate() {
        for &(callee, _) in edges.iter() {
            if let Some(slot) = reverse.get_mut(callee.0 as usize) {
                slot.push(caller);
            }
        }
    }
    let mut outdegree = vec![0usize; count];
    for (index, edges) in adjacency.iter().enumerate() {
        if let Some(slot) = outdegree.get_mut(index) {
            *slot = edges.len();
        }
    }
    let mut emitted = vec![false; count];
    let mut order: Vec<FunctionId> = Vec::new();
    loop {
        let mut picked = None;
        for index in 0..count {
            let ready = !emitted.get(index).copied().unwrap_or(true)
                && outdegree.get(index).copied().unwrap_or(1) == 0;
            if ready {
                picked = Some(index);
                break;
            }
        }
        let Some(index) = picked else {
            break;
        };
        set_flag(&mut emitted, index, true);
        order.push(FunctionId(index as u32));
        if let Some(callers) = reverse.get(index) {
            for &caller in callers.iter() {
                if let Some(slot) = outdegree.get_mut(caller) {
                    *slot = slot.saturating_sub(1);
                }
            }
        }
    }
    // Cyclic functions are rejected anyway; append in declaration order.
    for index in 0..count {
        if emitted.get(index).copied() != Some(true) {
            order.push(FunctionId(index as u32));
        }
    }
    // Longest chain (edge count) DP in topological order: call_order is
    // callees-first, so callees are final when their callers are processed.
    let mut chain = vec![0u32; count];
    for fid in &order {
        let Some(edges) = adjacency.get(fid.0 as usize) else {
            continue;
        };
        for &(callee, _) in edges.iter() {
            let child = chain.get(callee.0 as usize).copied().unwrap_or(0);
            let candidate = child.saturating_add(1);
            if let Some(slot) = chain.get_mut(fid.0 as usize) {
                if candidate > *slot {
                    *slot = candidate;
                }
            }
        }
    }
    let longest = chain.iter().copied().max().unwrap_or(0);
    if longest > c.limits.max_call_chain as u32 {
        let span = c.module.span;
        report(
            c,
            "B001",
            format!(
                "call chain exceeds the limit ({longest} > {})",
                c.limits.max_call_chain
            ),
            span,
        );
    }
    order
}

/// Colors the call graph and marks every node on any detected cycle.
fn find_cyclic(c: &mut Checker) -> Vec<bool> {
    let count = c.module.functions.len();
    let mut adjacency: Vec<Vec<(FunctionId, ExprId)>> = vec![Vec::new(); count];
    for edge in &c.edges {
        let caller = edge.caller.0 as usize;
        if let Some(edges) = adjacency.get_mut(caller) {
            edges.push((edge.callee, edge.site));
        }
    }
    let mut colors = vec![0u8; count];
    let mut cyclic = vec![false; count];
    for root in 0..count {
        if colors.get(root).copied().unwrap_or(2) != 0 {
            continue;
        }
        set_color(&mut colors, root, 1);
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some(top) = stack.pop() {
            let (node, mut cursor) = top;
            let mut descended = false;
            while let Some(&(callee, site)) =
                adjacency.get(node).and_then(|edges| edges.get(cursor))
            {
                cursor += 1;
                let state = colors.get(callee.0 as usize).copied().unwrap_or(2);
                if state == 0 {
                    set_color(&mut colors, callee.0 as usize, 1);
                    stack.push((node, cursor));
                    stack.push((callee.0 as usize, 0));
                    descended = true;
                    break;
                }
                if state == 1 {
                    report_cycle(c, site, callee);
                    for &(gray, _) in stack.iter() {
                        set_color(&mut colors, gray, 2);
                        mark_cyclic(&mut cyclic, gray);
                    }
                    set_color(&mut colors, node, 2);
                    mark_cyclic(&mut cyclic, node);
                    set_color(&mut colors, callee.0 as usize, 2);
                    mark_cyclic(&mut cyclic, callee.0 as usize);
                }
            }
            if descended {
                continue;
            }
            set_color(&mut colors, node, 2);
        }
    }
    cyclic
}

fn set_color(colors: &mut [u8], index: usize, color: u8) {
    if let Some(slot) = colors.get_mut(index) {
        *slot = color;
    }
}

fn mark_cyclic(cyclic: &mut [bool], index: usize) {
    if let Some(slot) = cyclic.get_mut(index) {
        *slot = true;
    }
}

fn set_flag(flags: &mut [bool], index: usize, value: bool) {
    if let Some(slot) = flags.get_mut(index) {
        *slot = value;
    }
}

/// Reports one recursion cycle at the back-edge call site.
fn report_cycle(c: &mut Checker, site: ExprId, start: FunctionId) {
    let primary = expr_span(c, site);
    let related = c
        .module
        .function(start)
        .map(|func| func.span)
        .unwrap_or(primary);
    report_related(
        c,
        "T008",
        "recursive call cycle detected".to_string(),
        primary,
        related,
        "first function of the recursion cycle",
    );
}

/// Adjacency restricted to edges whose endpoints are acyclic.
fn remaining_adjacency(c: &Checker, cyclic: &[bool]) -> Vec<Vec<(FunctionId, ExprId)>> {
    let count = c.module.functions.len();
    let mut adjacency: Vec<Vec<(FunctionId, ExprId)>> = vec![Vec::new(); count];
    for edge in &c.edges {
        let caller = edge.caller.0 as usize;
        let callee = edge.callee.0 as usize;
        let touches_cycle = cyclic.get(caller).copied().unwrap_or(true)
            || cyclic.get(callee).copied().unwrap_or(true);
        if touches_cycle {
            continue;
        }
        if let Some(edges) = adjacency.get_mut(caller) {
            edges.push((edge.callee, edge.site));
        }
    }
    adjacency
}

/// Computes transitive effects and checks every entry's capability ceiling.
pub(super) fn check_entry_capabilities(
    c: &mut Checker,
    call_order: &[FunctionId],
) -> Vec<EffectSet> {
    let count = c.module.functions.len();
    let cyclic = find_cyclic(c);
    let adjacency = remaining_adjacency(c, &cyclic);
    let mut merged: Vec<EffectSet> = (0..count)
        .map(|index| c.own_effects.get(index).copied().unwrap_or_default())
        .collect();
    for fid in call_order {
        let caller = fid.0 as usize;
        let Some(edges) = adjacency.get(caller) else {
            continue;
        };
        for &(callee, _) in edges.iter() {
            let borrowed = merged.get(callee.0 as usize).copied();
            if let (Some(slot), Some(effect)) = (merged.get_mut(caller), borrowed) {
                slot.union(effect);
            }
        }
    }
    for index in 0..count {
        let Some(kind) = c.entry_kinds.get(index).copied().flatten() else {
            continue;
        };
        check_entry(c, &adjacency, &merged, FunctionId(index as u32), kind);
    }
    merged
}

/// Breadth-first capability walk from one entry over remaining edges.
fn check_entry(
    c: &mut Checker,
    adjacency: &[Vec<(FunctionId, ExprId)>],
    merged: &[EffectSet],
    entry: FunctionId,
    kind: EntryKind,
) {
    let ceiling = capability_of(Some(kind));
    let count = c.module.functions.len();
    let mut visited = vec![false; count];
    let mut queue: VecDeque<usize> = VecDeque::new();
    if let Some(flag) = visited.get_mut(entry.0 as usize) {
        *flag = true;
    }
    queue.push_back(entry.0 as usize);
    while let Some(node) = queue.pop_front() {
        let Some(edges) = adjacency.get(node) else {
            continue;
        };
        for &(callee, site) in edges.iter() {
            let introduced = merged.get(callee.0 as usize).copied().unwrap_or_default();
            if introduced.validator_ctors() && !ceiling.permits(BuiltinCategory::ValidatorCtor) {
                let related = c
                    .module
                    .function(callee)
                    .map(|func| func.span)
                    .unwrap_or_else(|| expr_span(c, site));
                report_related(
                    c,
                    "T007",
                    "helper effects exceed the entry capability".to_string(),
                    expr_span(c, site),
                    related,
                    "function with validator-constructor effects",
                );
            }
            if introduced.stage2() && !ceiling.permits(BuiltinCategory::Stage2) {
                let related = c
                    .module
                    .function(callee)
                    .map(|func| func.span)
                    .unwrap_or_else(|| expr_span(c, site));
                report_related(
                    c,
                    "T007",
                    "helper effects exceed the entry capability".to_string(),
                    expr_span(c, site),
                    related,
                    "function with database effects",
                );
            }
            let seen = visited.get(callee.0 as usize).copied().unwrap_or(true);
            if !seen {
                if let Some(flag) = visited.get_mut(callee.0 as usize) {
                    *flag = true;
                }
                queue.push_back(callee.0 as usize);
            }
        }
    }
}
