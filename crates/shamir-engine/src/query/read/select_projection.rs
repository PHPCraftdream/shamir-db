//! Pre-resolved SELECT projection — avoids re-interning paths per record.

use serde_json as json;

use crate::query::filter::eval::{intern_field_path, resolve_field_ref, resolve_filter_value};
use crate::query::filter::{FilterContext, FilterValue, FnCall};
use crate::query::read::{QueryResult, Select, SelectItem};
use shamir_types::codecs::interned::{inner_to_json_value, inner_value_to_query_value};
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::InnerValue;
use shamir_types::types::value::QueryValue;

/// Pre-resolved select projection info (avoids re-interning paths per record).
///
/// Output keys (alias or last path segment) are pre-allocated as
/// `String` at compile time — `project()` clones them per record
/// instead of paying `to_string()` for each field on each row.
pub struct SelectProjection {
    /// true → just convert whole record to JSON
    pub(super) is_all: bool,
    /// (interned_path, pre-built output key)
    pub(super) fields: Vec<(Option<Vec<u64>>, String)>,
    /// Scalar-function projections: (output key, FnCall-shaped FilterValue).
    /// Evaluated per record via `resolve_filter_value`, reusing the filter
    /// value model (`$ref` / literals / nested `$fn`).
    pub(super) funcs: Vec<(String, FilterValue)>,
    /// Empty resolved-refs map so `project` can build a `FilterContext`
    /// without `$query` support (projection scalar fns see only the row).
    pub(super) empty_refs: TMap<String, QueryResult>,
}

impl SelectProjection {
    /// Build a reusable projection from a Select + Interner.
    pub fn new(select: &Select, interner: &Interner) -> Self {
        let is_all =
            select.items.is_empty() || select.items.iter().any(|i| matches!(i, SelectItem::All));

        let (fields, funcs) = if is_all {
            (Vec::new(), Vec::new())
        } else {
            let mut fields = Vec::new();
            let mut funcs = Vec::new();
            for item in &select.items {
                match item {
                    SelectItem::Field { path, alias } => {
                        let interned = intern_field_path(path, interner);
                        let key = alias
                            .clone()
                            .unwrap_or_else(|| path.last().cloned().unwrap_or_default());
                        fields.push((interned, key));
                    }
                    SelectItem::Function { name, args, alias } => {
                        let key = alias.clone().unwrap_or_else(|| name.clone());
                        let fv = FilterValue::FnCall {
                            call: FnCall::complex(name.clone(), args.clone()),
                        };
                        funcs.push((key, fv));
                    }
                    _ => {}
                }
            }
            (fields, funcs)
        };

        Self {
            is_all,
            fields,
            funcs,
            empty_refs: new_map_wc(0),
        }
    }

    /// Project a single InnerValue record to JSON.
    pub fn project(&self, record: &InnerValue, interner: &Interner) -> json::Value {
        if self.is_all {
            return inner_to_json_value(record, interner).unwrap_or(json::Value::Null);
        }
        if self.fields.is_empty() && self.funcs.is_empty() {
            return json::Value::Object(json::Map::new());
        }
        let mut obj = json::Map::new();
        for (interned_path, key) in &self.fields {
            let val = interned_path
                .as_ref()
                .and_then(|p| resolve_field_ref(record, p))
                .map(|v| inner_to_json_value(v, interner).unwrap_or(json::Value::Null))
                .unwrap_or(json::Value::Null);
            obj.insert(key.clone(), val);
        }
        if !self.funcs.is_empty() {
            let ctx = FilterContext::new(interner, &self.empty_refs);
            for (key, fv) in &self.funcs {
                let val = resolve_filter_value(fv, record, &ctx)
                    .map(|v| inner_to_json_value(&v, interner).unwrap_or(json::Value::Null))
                    .unwrap_or(json::Value::Null);
                obj.insert(key.clone(), val);
            }
        }
        json::Value::Object(obj)
    }

    /// Project a single InnerValue record to QueryValue.
    ///
    /// Mirrors `project` exactly — same branching, same field/func
    /// handling — but builds `QueryValue` (string-keyed) instead of
    /// `serde_json::Value`.  Callers switch to this once the read path
    /// stops needing `serde_json`.
    pub fn project_value(&self, record: &InnerValue, interner: &Interner) -> QueryValue {
        if self.is_all {
            return inner_value_to_query_value(record, interner).unwrap_or(QueryValue::Null);
        }
        if self.fields.is_empty() && self.funcs.is_empty() {
            return QueryValue::Map(shamir_types::types::common::new_map_wc(0));
        }
        let mut obj = shamir_types::types::common::new_map_wc(self.fields.len() + self.funcs.len());
        for (interned_path, key) in &self.fields {
            let val = interned_path
                .as_ref()
                .and_then(|p| resolve_field_ref(record, p))
                .map(|v| inner_value_to_query_value(v, interner).unwrap_or(QueryValue::Null))
                .unwrap_or(QueryValue::Null);
            obj.insert(key.clone(), val);
        }
        if !self.funcs.is_empty() {
            let ctx = FilterContext::new(interner, &self.empty_refs);
            for (key, fv) in &self.funcs {
                let val = resolve_filter_value(fv, record, &ctx)
                    .map(|v| inner_value_to_query_value(&v, interner).unwrap_or(QueryValue::Null))
                    .unwrap_or(QueryValue::Null);
                obj.insert(key.clone(), val);
            }
        }
        QueryValue::Map(obj)
    }
}
