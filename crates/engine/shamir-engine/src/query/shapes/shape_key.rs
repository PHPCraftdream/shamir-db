use std::fmt::Write as _;
use std::hash::Hasher;

use rustc_hash::FxHasher;
use shamir_query_types::filter::{Filter, FilterExprOp, FilterValue, ValueCompareOp};
use shamir_query_types::read::filter::MAX_FILTER_DEPTH;
use shamir_query_types::read::{
    AggFunc, AggregateField, GroupBy, NullsOrder, OrderBy, OrderDirection, Pagination, ReadQuery,
    Select, SelectExpr, SelectExprValue, SelectItem,
};
use shamir_types::core::interner::Interner;
use shamir_types::types::value::QueryValue;

use crate::query::batch::BatchOp;

const SEED_A: u64 = 0x9e3779b97f4a7c15;
const SEED_B: u64 = 0xc2b2ae3d27d4eb4f;
const TOO_DEEP: u8 = 0xFE;
const SELECT_END: u8 = 0xFF;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// Canonical key of one data-op execution's shape.
///
/// Two executions fold to the same key when they differ only in literal
/// values, patterns, aliases, parameter names, catalog lookups or limit
/// values — everything that does not change the work the engine performs.
/// Unknown field names deliberately collapse into a single key so they
/// neither grow the interner nor multiply shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeKey([u8; 16]);

impl ShapeKey {
    /// Fold one data operation under `table`'s catalog name.
    pub fn fold(op: &BatchOp, table: &str, interner: &Interner) -> ShapeKey {
        let mut fold = Fold::new();
        match op {
            BatchOp::Read(query) => {
                fold.tag(1);
                fold.name(table);
                fold_read_query(&mut fold, query, interner);
            }
            BatchOp::Insert(_) => {
                fold.tag(2);
                fold.name(table);
            }
            BatchOp::Update(op) => {
                fold.tag(3);
                fold.name(table);
                fold_optional_filter(&mut fold, op.where_clause.as_ref(), interner, 1);
                fold_update_set(&mut fold, &op.set, interner);
            }
            BatchOp::Delete(op) => {
                fold.tag(4);
                fold.name(table);
                fold_filter(&mut fold, &op.where_clause, interner, 1);
            }
            BatchOp::Set(_) => {
                fold.tag(5);
                fold.name(table);
            }
            _ => {}
        }
        ShapeKey(fold.finish())
    }

    /// Raw key bytes, suitable for a map key or a metrics registry.
    pub fn into_bytes(self) -> [u8; 16] {
        self.0
    }

    /// Lowercase hex rendering of the raw key (32 characters).
    pub fn to_hex(&self) -> String {
        let mut text = String::with_capacity(32);
        for byte in self.0 {
            let _ = write!(&mut text, "{byte:02x}");
        }
        text
    }
}

/// Incremental polynomial fold over two independently seeded `FxHasher`s.
/// Byte-level `write` is never used: every input is a `u8` tag or `u64` word.
struct Fold {
    a: FxHasher,
    b: FxHasher,
}

impl Fold {
    fn new() -> Self {
        Self {
            a: FxHasher::with_seed(SEED_A as usize),
            b: FxHasher::with_seed(SEED_B as usize),
        }
    }

    fn tag(&mut self, value: u8) {
        self.a.write_u8(value);
        self.b.write_u8(value);
    }

    fn num(&mut self, value: u64) {
        self.a.write_u64(value);
        self.b.write_u64(value);
    }

    fn name(&mut self, value: &str) {
        self.num(fnv1a64(value.as_bytes()));
    }

    fn finish(self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&self.a.finish().to_be_bytes());
        bytes[8..].copy_from_slice(&self.b.finish().to_be_bytes());
        bytes
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn fold_read_query(fold: &mut Fold, query: &ReadQuery, interner: &Interner) {
    fold_select(fold, &query.select, interner, 1);
    fold_optional_filter(fold, query.r#where.as_ref(), interner, 1);
    fold_group_by(fold, query.group_by.as_ref(), interner);
    fold_order_by(fold, query.order_by.as_ref(), interner);
    fold_pagination(fold, &query.pagination);
}

fn fold_select(fold: &mut Fold, select: &Select, interner: &Interner, depth: usize) {
    if select.items.len() == 1
        && matches!(select.items.first(), Some(SelectItem::All))
        && !select.distinct
    {
        fold.tag(0);
        return;
    }
    fold.tag(1);
    for item in &select.items {
        fold_select_item(fold, item, interner, depth);
    }
    fold.tag(SELECT_END);
    fold.tag(u8::from(select.distinct));
}

fn fold_select_item(fold: &mut Fold, item: &SelectItem, interner: &Interner, depth: usize) {
    match item {
        SelectItem::All => fold.tag(0),
        SelectItem::Field { path, .. } => {
            fold.tag(1);
            fold_field_path(fold, path, interner);
        }
        SelectItem::Aggregate {
            func,
            field,
            input,
            distinct,
            ..
        } => {
            fold.tag(2);
            fold.tag(match func {
                AggFunc::Count => 1,
                AggFunc::Sum => 2,
                AggFunc::Avg => 3,
                AggFunc::Min => 4,
                AggFunc::Max => 5,
            });
            fold_aggregate_field(fold, field, interner);
            fold_optional_value(fold, input.as_ref(), interner, depth + 1);
            fold.tag(u8::from(*distinct));
        }
        SelectItem::CountAll { .. } => fold.tag(3),
        SelectItem::AggregateFn {
            name,
            field,
            input,
            args,
            ..
        } => {
            fold.tag(4);
            fold.name(name);
            fold_aggregate_field(fold, field, interner);
            fold_optional_value(fold, input.as_ref(), interner, depth + 1);
            fold.num(args.len() as u64);
            for arg in args {
                fold_filter_value(fold, arg, interner, depth + 1);
            }
        }
        SelectItem::Function { name, args, .. } => {
            fold.tag(5);
            fold.name(name);
            fold.num(args.len() as u64);
            for arg in args {
                fold_filter_value(fold, arg, interner, depth + 1);
            }
        }
        SelectItem::Expression { expr, .. } => {
            fold.tag(6);
            fold_select_expr(fold, expr, interner, depth + 1);
        }
    }
}

fn fold_aggregate_field(fold: &mut Fold, field: &AggregateField, interner: &Interner) {
    match field {
        AggregateField::Field(path) => {
            fold.tag(1);
            fold_field_path(fold, path, interner);
        }
        AggregateField::All => fold.tag(0),
    }
}

fn fold_optional_value(
    fold: &mut Fold,
    value: Option<&FilterValue>,
    interner: &Interner,
    depth: usize,
) {
    match value {
        Some(value) => {
            fold.tag(1);
            fold_filter_value(fold, value, interner, depth);
        }
        None => fold.tag(0),
    }
}

fn fold_group_by(fold: &mut Fold, group: Option<&GroupBy>, interner: &Interner) {
    match group {
        Some(group) => {
            fold.tag(1);
            fold.num(group.fields.len() as u64);
            for path in &group.fields {
                fold_field_path(fold, path, interner);
            }
            fold_optional_filter(fold, group.having.as_ref(), interner, 2);
        }
        None => fold.tag(0),
    }
}

fn fold_order_by(fold: &mut Fold, order: Option<&OrderBy>, interner: &Interner) {
    match order {
        Some(order) => {
            fold.tag(1);
            fold.num(order.items.len() as u64);
            for item in &order.items {
                fold_field_path(fold, &item.field, interner);
                fold.tag(match item.direction {
                    OrderDirection::Asc => 0,
                    OrderDirection::Desc => 1,
                });
                fold.tag(match item.nulls {
                    None => 0,
                    Some(NullsOrder::First) => 1,
                    Some(NullsOrder::Last) => 2,
                });
            }
        }
        None => fold.tag(0),
    }
}

fn fold_pagination(fold: &mut Fold, pagination: &Pagination) {
    match pagination {
        Pagination::None => fold.tag(0),
        Pagination::LimitOffset { limit, offset } => {
            fold.tag(1);
            fold.tag(u8::from(limit.is_some()));
            fold.tag(u8::from(*offset != 0));
        }
        Pagination::Page { page, page_size } => {
            fold.tag(2);
            fold.tag(u8::from(*page != 0));
            fold.tag(u8::from(*page_size != 0));
        }
        Pagination::After { limit, .. } => {
            fold.tag(3);
            fold.tag(u8::from(limit.is_some()));
        }
    }
}

fn fold_optional_filter(
    fold: &mut Fold,
    filter: Option<&Filter>,
    interner: &Interner,
    depth: usize,
) {
    match filter {
        Some(filter) => {
            fold.tag(1);
            fold_filter(fold, filter, interner, depth);
        }
        None => fold.tag(0),
    }
}

fn fold_update_set(fold: &mut Fold, set: &QueryValue, interner: &Interner) {
    if let QueryValue::Map(fields) = set {
        for key in fields.keys() {
            match interner.get_ind(key) {
                Some(id) => {
                    fold.tag(0);
                    fold.num(1);
                    fold.num(id.id());
                }
                None => fold.tag(1),
            }
        }
    }
}

fn fold_filter(fold: &mut Fold, filter: &Filter, interner: &Interner, depth: usize) {
    if depth > MAX_FILTER_DEPTH {
        fold.tag(TOO_DEEP);
        return;
    }
    match filter {
        Filter::Eq { field, value } => {
            fold.tag(1);
            fold_field_path(fold, field, interner);
            fold_filter_value(fold, value, interner, depth + 1);
        }
        Filter::Ne { field, value } => {
            fold.tag(2);
            fold_field_path(fold, field, interner);
            fold_filter_value(fold, value, interner, depth + 1);
        }
        Filter::Gt { field, value } => {
            fold.tag(3);
            fold_field_path(fold, field, interner);
            fold_filter_value(fold, value, interner, depth + 1);
        }
        Filter::Gte { field, value } => {
            fold.tag(4);
            fold_field_path(fold, field, interner);
            fold_filter_value(fold, value, interner, depth + 1);
        }
        Filter::Lt { field, value } => {
            fold.tag(5);
            fold_field_path(fold, field, interner);
            fold_filter_value(fold, value, interner, depth + 1);
        }
        Filter::Lte { field, value } => {
            fold.tag(6);
            fold_field_path(fold, field, interner);
            fold_filter_value(fold, value, interner, depth + 1);
        }
        Filter::Like { field, .. } => {
            fold.tag(7);
            fold_field_path(fold, field, interner);
            fold.tag(0);
        }
        Filter::ILike { field, .. } => {
            fold.tag(8);
            fold_field_path(fold, field, interner);
            fold.tag(0);
        }
        Filter::Regex { field, .. } => {
            fold.tag(9);
            fold_field_path(fold, field, interner);
            fold.tag(0);
        }
        Filter::IsNull { field } => {
            fold.tag(10);
            fold_field_path(fold, field, interner);
        }
        Filter::IsNotNull { field } => {
            fold.tag(11);
            fold_field_path(fold, field, interner);
        }
        Filter::In { field, values } => {
            fold.tag(12);
            fold_field_path(fold, field, interner);
            fold_filter_values(fold, values, interner, depth);
        }
        Filter::NotIn { field, values } => {
            fold.tag(13);
            fold_field_path(fold, field, interner);
            fold_filter_values(fold, values, interner, depth);
        }
        Filter::Contains { field, value } => {
            fold.tag(14);
            fold_field_path(fold, field, interner);
            fold_filter_value(fold, value, interner, depth + 1);
        }
        Filter::ContainsAny { field, values } => {
            fold.tag(15);
            fold_field_path(fold, field, interner);
            fold_filter_values(fold, values, interner, depth);
        }
        Filter::ContainsAll { field, values } => {
            fold.tag(16);
            fold_field_path(fold, field, interner);
            fold_filter_values(fold, values, interner, depth);
        }
        Filter::Between { field, from, to } => {
            fold.tag(17);
            fold_field_path(fold, field, interner);
            fold_filter_value(fold, from, interner, depth + 1);
            fold_filter_value(fold, to, interner, depth + 1);
        }
        Filter::Exists { field } => {
            fold.tag(18);
            fold_field_path(fold, field, interner);
        }
        Filter::NotExists { field } => {
            fold.tag(19);
            fold_field_path(fold, field, interner);
        }
        Filter::And { filters } => {
            fold.tag(20);
            fold.num(filters.len() as u64);
            for filter in filters {
                fold_filter(fold, filter, interner, depth + 1);
            }
        }
        Filter::Or { filters } => {
            fold.tag(21);
            fold.num(filters.len() as u64);
            for filter in filters {
                fold_filter(fold, filter, interner, depth + 1);
            }
        }
        Filter::Not { filter } => {
            fold.tag(22);
            fold_filter(fold, filter, interner, depth + 1);
        }
        Filter::FieldEq { field, value } => {
            fold.tag(23);
            fold_field_path(fold, field, interner);
            fold_filter_value(fold, value, interner, depth + 1);
        }
        Filter::ValueCompare { left, cmp, right } => {
            fold.tag(24);
            fold_filter_value(fold, left, interner, depth + 1);
            fold.tag(match cmp {
                ValueCompareOp::Eq => 1,
                ValueCompareOp::Ne => 2,
                ValueCompareOp::Gt => 3,
                ValueCompareOp::Gte => 4,
                ValueCompareOp::Lt => 5,
                ValueCompareOp::Lte => 6,
            });
            fold_filter_value(fold, right, interner, depth + 1);
        }
        Filter::Fts {
            field, mode, limit, ..
        } => {
            fold.tag(25);
            fold_field_path(fold, field, interner);
            fold.tag(u8::from(mode == "and"));
            fold.tag(u8::from(limit.is_some()));
        }
        Filter::VectorSimilarity { field, .. } => {
            fold.tag(26);
            fold_field_path(fold, field, interner);
            fold.tag(0);
        }
        Filter::Computed {
            expr_op,
            field,
            expr_args,
            cmp,
            value,
        } => {
            fold.tag(27);
            fold.name(expr_op);
            fold_field_path(fold, field, interner);
            match expr_args {
                Some(args) => {
                    fold.tag(1);
                    fold.num(args.len() as u64);
                    for arg in args {
                        fold_filter_value(fold, arg, interner, depth + 1);
                    }
                }
                None => fold.tag(0),
            }
            fold.name(cmp);
            fold_filter_value(fold, value, interner, depth + 1);
        }
    }
}

fn fold_filter_values(fold: &mut Fold, values: &[FilterValue], interner: &Interner, depth: usize) {
    fold.num(values.len() as u64);
    for value in values {
        fold_filter_value(fold, value, interner, depth + 1);
    }
}

fn fold_filter_value(fold: &mut Fold, value: &FilterValue, interner: &Interner, depth: usize) {
    if depth > MAX_FILTER_DEPTH {
        fold.tag(TOO_DEEP);
        return;
    }
    match value {
        FilterValue::Null => fold.tag(1),
        FilterValue::Bool(_) => fold.tag(2),
        FilterValue::Int(_) => fold.tag(3),
        FilterValue::Float(_) => fold.tag(4),
        FilterValue::String(_) => fold.tag(5),
        FilterValue::Binary(_) => fold.tag(6),
        FilterValue::Array(items) => {
            fold.tag(7);
            fold.num(items.len() as u64);
            for item in items {
                fold_filter_value(fold, item, interner, depth + 1);
            }
        }
        FilterValue::FieldRef { path } => {
            fold.tag(8);
            fold_field_path(fold, path, interner);
        }
        FilterValue::QueryRef { .. } => fold.tag(9),
        FilterValue::FnCall { call } => {
            fold.tag(10);
            fold.name(call.name());
            fold.num(call.args().len() as u64);
        }
        FilterValue::Expr { expr } => {
            fold.tag(11);
            fold.tag(match expr.op {
                FilterExprOp::Add => 1,
                FilterExprOp::Sub => 2,
                FilterExprOp::Mul => 3,
                FilterExprOp::Div => 4,
                FilterExprOp::Mod => 5,
                FilterExprOp::Neg => 6,
                FilterExprOp::Concat => 7,
                FilterExprOp::Lower => 8,
                FilterExprOp::Upper => 9,
                FilterExprOp::Trim => 10,
                FilterExprOp::Length => 11,
                FilterExprOp::And => 12,
                FilterExprOp::Or => 13,
                FilterExprOp::Not => 14,
                FilterExprOp::Eq => 15,
                FilterExprOp::Ne => 16,
                FilterExprOp::Gt => 17,
                FilterExprOp::Gte => 18,
                FilterExprOp::Lt => 19,
                FilterExprOp::Lte => 20,
            });
            fold.num(expr.args.len() as u64);
            for arg in &expr.args {
                fold_filter_value(fold, arg, interner, depth + 1);
            }
        }
        FilterValue::Cond { cond } => {
            fold.tag(12);
            fold_filter(fold, cond.condition.as_ref(), interner, depth + 1);
            fold_filter_value(fold, &cond.then, interner, depth + 1);
            fold_filter_value(fold, &cond.or_else, interner, depth + 1);
        }
        FilterValue::Param { .. } => fold.tag(13),
    }
}

fn fold_select_expr(fold: &mut Fold, expr: &SelectExpr, interner: &Interner, depth: usize) {
    if depth > MAX_FILTER_DEPTH {
        fold.tag(TOO_DEEP);
        return;
    }
    match expr {
        SelectExpr::Add { left, right } => {
            fold.tag(1);
            fold_select_expr(fold, left, interner, depth + 1);
            fold_select_expr(fold, right, interner, depth + 1);
        }
        SelectExpr::Sub { left, right } => {
            fold.tag(2);
            fold_select_expr(fold, left, interner, depth + 1);
            fold_select_expr(fold, right, interner, depth + 1);
        }
        SelectExpr::Mul { left, right } => {
            fold.tag(3);
            fold_select_expr(fold, left, interner, depth + 1);
            fold_select_expr(fold, right, interner, depth + 1);
        }
        SelectExpr::Div { left, right } => {
            fold.tag(4);
            fold_select_expr(fold, left, interner, depth + 1);
            fold_select_expr(fold, right, interner, depth + 1);
        }
        SelectExpr::Field { path } => {
            fold.tag(5);
            fold_field_path(fold, path, interner);
        }
        SelectExpr::Literal { value } => {
            fold.tag(6);
            fold.tag(match value {
                SelectExprValue::Null => 1,
                SelectExprValue::Bool(_) => 2,
                SelectExprValue::Int(_) => 3,
                SelectExprValue::Float(_) => 4,
                SelectExprValue::String(_) => 5,
            });
        }
    }
}

fn fold_field_path(fold: &mut Fold, path: &[String], interner: &Interner) {
    if path
        .iter()
        .any(|segment| interner.get_ind(segment).is_none())
    {
        fold.tag(1);
        return;
    }
    fold.tag(0);
    fold.num(path.len() as u64);
    for segment in path {
        if let Some(key) = interner.get_ind(segment) {
            fold.num(key.id());
        }
    }
}
