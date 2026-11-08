use std::collections::BTreeMap;

use crate::front::{ExprId, ExprKind, FunctionId, StatementId, StatementKind};
use crate::sem::effects::builtin_by_name;
use crate::sem::tables::{Callee, ResolvedCall, ResolvedTarget};

use super::entry::{expr_span, report, CallEdge, Checker};

/// Resolves every name, call target and binding slot for all functions.
pub(super) fn resolve_module(c: &mut Checker) {
    let count = c.module.functions.len();
    for index in 0..count {
        let fid = FunctionId(index as u32);
        let func = match c.module.function(fid) {
            Some(func) => func,
            None => continue,
        };
        let param_count = match c.signatures.get(index) {
            Some(sig) => sig.params.len() as u32,
            None => func.parameters.len() as u32,
        };
        if let Some(mutability) = c.slot_mutability.get_mut(fid.0 as usize) {
            let params_immutable = vec![false; param_count as usize];
            *mutability = params_immutable;
        }
        let mut resolver = FnResolver {
            fid,
            param_count,
            next_slot: param_count,
            scopes: vec![BTreeMap::new()],
            locals_capped: false,
        };
        resolver.register_parameters(c);
        resolver.walk_statement(c, func.body, 0);
    }
}

/// Per-function scope tracker; combined slot space, parameters first.
struct FnResolver {
    fid: FunctionId,
    param_count: u32,
    next_slot: u32,
    scopes: Vec<BTreeMap<String, u32>>,
    locals_capped: bool,
}

impl FnResolver {
    /// Registers passing parameter names in the outermost scope.
    fn register_parameters(&mut self, c: &mut Checker) {
        let Some(func) = c.module.function(self.fid) else {
            return;
        };
        let param_ids = func.parameters.clone();
        for (position, pid) in param_ids.iter().enumerate() {
            let Some(param) = c.module.parameter(*pid) else {
                continue;
            };
            let name = param.name.clone();
            if builtin_by_name(&name).is_some() || name == "args" {
                continue;
            }
            let scope = match self.scopes.first_mut() {
                Some(scope) => scope,
                None => continue,
            };
            if scope.contains_key(&name) {
                continue;
            }
            let slot = match u32::try_from(position) {
                Ok(slot) => slot,
                Err(_) => continue,
            };
            scope.insert(name, slot);
        }
    }

    /// Walks one statement, registering bindings and resolving names.
    fn walk_statement(&mut self, c: &mut Checker, stmt: StatementId, depth: u32) {
        if depth > c.limits.max_ast_depth as u32 {
            let span = c
                .module
                .statement(stmt)
                .map(|node| node.span)
                .unwrap_or(c.module.span);
            report(
                c,
                "B001",
                "semantic resolution nesting exceeds the limit".to_string(),
                span,
            );
            return;
        }
        let kind = match c.module.statement(stmt) {
            Some(stmt) => stmt.kind.clone(),
            None => return,
        };
        match kind {
            StatementKind::Block(inner) => {
                self.scopes.push(BTreeMap::new());
                for child in inner {
                    self.walk_statement(c, child, depth.saturating_add(1));
                }
                self.scopes.pop();
            }
            StatementKind::Let {
                mutable,
                name,
                value,
                ..
            } => {
                self.resolve_expression(c, value, depth);
                self.declare_local(c, stmt, name, mutable);
            }
            StatementKind::Assign { target, value } => {
                self.resolve_expression(c, target, depth);
                self.resolve_expression(c, value, depth);
            }
            StatementKind::If {
                condition,
                then_block,
                else_branch,
            } => {
                self.resolve_expression(c, condition, depth);
                self.walk_statement(c, then_block, depth.saturating_add(1));
                if let Some(else_block) = else_branch {
                    self.walk_statement(c, else_block, depth.saturating_add(1));
                }
            }
            StatementKind::While { condition, body } => {
                self.resolve_expression(c, condition, depth);
                self.walk_statement(c, body, depth.saturating_add(1));
            }
            StatementKind::For {
                name,
                start_or_list,
                end,
                body,
                ..
            } => {
                self.resolve_expression(c, start_or_list, depth);
                if let Some(end_expr) = end {
                    self.resolve_expression(c, end_expr, depth);
                }
                self.scopes.push(BTreeMap::new());
                self.declare_local(c, stmt, name, false);
                self.walk_statement(c, body, depth.saturating_add(1));
                self.scopes.pop();
            }
            StatementKind::Return(value) => {
                if let Some(value) = value {
                    self.resolve_expression(c, value, depth);
                }
            }
            StatementKind::Break | StatementKind::Continue => {}
            StatementKind::Throw(value) => self.resolve_expression(c, value, depth),
            StatementKind::Expression(expr) => self.resolve_expression(c, expr, depth),
        }
    }

    /// Declares one binding in the current scope; reports duplicates.
    fn declare_local(&mut self, c: &mut Checker, stmt: StatementId, name: String, mutable: bool) {
        if builtin_by_name(&name).is_some() {
            let span = c
                .module
                .statement(stmt)
                .map(|stmt| stmt.span)
                .unwrap_or(c.module.span);
            report(c, "N002", format!("local name {name} is reserved"), span);
            return;
        }
        let duplicate = self
            .scopes
            .last()
            .map(|scope| scope.contains_key(&name))
            .unwrap_or(true);
        if duplicate {
            let span = c
                .module
                .statement(stmt)
                .map(|stmt| stmt.span)
                .unwrap_or(c.module.span);
            report(c, "N002", format!("duplicate local {name}"), span);
            return;
        }
        let locals_after = self
            .next_slot
            .saturating_sub(self.param_count)
            .saturating_add(1);
        if locals_after > c.limits.max_locals_per_function as u32 && !self.locals_capped {
            self.locals_capped = true;
            let span = c
                .module
                .function(self.fid)
                .map(|func| func.span)
                .unwrap_or(c.module.span);
            report(c, "B001", "too many locals".to_string(), span);
        }
        let slot = self.next_slot;
        self.next_slot = slot.saturating_add(1);
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, slot);
        }
        if let Some(slot_entry) = c.binding_slots.get_mut(stmt.0 as usize) {
            *slot_entry = Some(slot);
        }
        if let Some(mutability) = c.slot_mutability.get_mut(self.fid.0 as usize) {
            mutability.push(mutable);
        }
    }

    /// Resolves one expression and all descendants.
    fn resolve_expression(&mut self, c: &mut Checker, id: ExprId, depth: u32) {
        if depth > c.limits.max_ast_depth as u32 {
            let span = c
                .module
                .expression(id)
                .map(|node| node.span)
                .unwrap_or(c.module.span);
            report(
                c,
                "B001",
                "semantic resolution nesting exceeds the limit".to_string(),
                span,
            );
            return;
        }
        let kind = match c.module.expression(id) {
            Some(expr) => expr.kind.clone(),
            None => return,
        };
        match kind {
            ExprKind::Identifier(name) => {
                let target = self.resolve_identifier(c, id, &name);
                self.store_resolved(c, id, target);
            }
            ExprKind::Integer { .. }
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Bytes(_)
            | ExprKind::Boolean(_)
            | ExprKind::Null => {}
            ExprKind::Group(inner) => self.resolve_expression(c, inner, depth.saturating_add(1)),
            ExprKind::List(items) => {
                for item in items {
                    self.resolve_expression(c, item, depth.saturating_add(1));
                }
            }
            ExprKind::Record(fields) => {
                for field_id in fields {
                    if let Some(field) = c.module.value_field(field_id) {
                        let value = field.value;
                        self.resolve_expression(c, value, depth.saturating_add(1));
                    }
                }
            }
            ExprKind::Unary { operand, .. } => {
                self.resolve_expression(c, operand, depth.saturating_add(1))
            }
            ExprKind::Binary { left, right, .. } => {
                self.resolve_expression(c, left, depth.saturating_add(1));
                self.resolve_expression(c, right, depth.saturating_add(1));
            }
            ExprKind::Conditional {
                condition,
                then_value,
                else_value,
            } => {
                self.resolve_expression(c, condition, depth.saturating_add(1));
                self.resolve_expression(c, then_value, depth.saturating_add(1));
                self.resolve_expression(c, else_value, depth.saturating_add(1));
            }
            ExprKind::Call { target, arguments } => {
                let callee = self.resolve_call_target(c, id, target, depth);
                for argument in arguments {
                    self.resolve_expression(c, argument, depth.saturating_add(1));
                }
                let entry = c.calls.get_mut(id.0 as usize);
                if let Some(slot) = entry {
                    *slot = Some(ResolvedCall { callee });
                }
            }
            ExprKind::Field { target, .. } => {
                self.resolve_expression(c, target, depth.saturating_add(1))
            }
            ExprKind::Index { target, index } => {
                self.resolve_expression(c, target, depth.saturating_add(1));
                self.resolve_expression(c, index, depth.saturating_add(1));
            }
        }
    }

    /// Resolves a bare name to a parameter, local, function or builtin.
    fn resolve_identifier(&mut self, c: &mut Checker, id: ExprId, name: &str) -> ResolvedTarget {
        for scope in self.scopes.iter().rev() {
            if let Some(&slot) = scope.get(name) {
                return if slot < self.param_count {
                    ResolvedTarget::Parameter(slot)
                } else {
                    ResolvedTarget::Local(slot)
                };
            }
        }
        if let Some(&fid) = c.by_name.get(name) {
            return ResolvedTarget::Function(fid);
        }
        if let Some(bid) = builtin_by_name(name) {
            return ResolvedTarget::Builtin(bid);
        }
        let span = expr_span(c, id);
        report(c, "N001", format!("unresolved name {name}"), span);
        ResolvedTarget::Error
    }

    fn store_resolved(&mut self, c: &mut Checker, id: ExprId, target: ResolvedTarget) {
        if let Some(slot) = c.resolved.get_mut(id.0 as usize) {
            *slot = Some(target);
        }
    }

    /// Resolves a call target and records the callee plus any call edge.
    fn resolve_call_target(
        &mut self,
        c: &mut Checker,
        call_id: ExprId,
        target: ExprId,
        depth: u32,
    ) -> Callee {
        let name = match c.module.expression(target) {
            Some(expr) => match &expr.kind {
                ExprKind::Identifier(name) => Some(name.clone()),
                ExprKind::Integer { .. }
                | ExprKind::Float(_)
                | ExprKind::String(_)
                | ExprKind::Bytes(_)
                | ExprKind::Boolean(_)
                | ExprKind::Null
                | ExprKind::Group(_)
                | ExprKind::List(_)
                | ExprKind::Record(_)
                | ExprKind::Unary { .. }
                | ExprKind::Binary { .. }
                | ExprKind::Conditional { .. }
                | ExprKind::Call { .. }
                | ExprKind::Field { .. }
                | ExprKind::Index { .. } => None,
            },
            None => None,
        };
        let resolved_target = match name {
            Some(name) => {
                let resolved = self.resolve_identifier(c, target, &name);
                self.store_resolved(c, target, resolved);
                resolved
            }
            None => {
                self.resolve_expression(c, target, depth.saturating_add(1));
                ResolvedTarget::Error
            }
        };
        match resolved_target {
            ResolvedTarget::Function(callee_fid) => {
                self.record_edge(c, callee_fid, call_id);
                Callee::Function(callee_fid)
            }
            ResolvedTarget::Builtin(bid) => Callee::Builtin(bid),
            ResolvedTarget::Parameter(_) | ResolvedTarget::Local(_) => {
                let span = expr_span(c, target);
                report(
                    c,
                    "T004",
                    "call target is not a function or builtin".to_string(),
                    span,
                );
                Callee::Error
            }
            ResolvedTarget::Error => Callee::Error,
        }
    }

    fn record_edge(&mut self, c: &mut Checker, callee: FunctionId, site: ExprId) {
        c.edges.push(CallEdge {
            caller: self.fid,
            callee,
            site,
        });
    }
}
