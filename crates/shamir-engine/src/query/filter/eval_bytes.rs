//! Bytes-level pre-filter for msgpack records.
//!
//! `FilterNode::matches_msgpack_bytes` walks raw msgpack without decoding the
//! full record into `InnerValue`.  It is a **fast-skip pre-filter** — it
//! never produces a false-negative or a false-positive:
//!
//! - `Some(false)` — filter definitely rejects the row; caller skips it (no
//!   full decode needed → the win).
//! - `Some(true)` — filter definitely accepts; caller still does the full
//!   decode + normal filter as a safety net (cheap on the happy path).
//! - `None` — filter shape is too complex for bytes-eval; caller falls through
//!   to the full decode + normal filter unchanged.
//!
//! Only simple atoms that can be evaluated by inspecting raw msgpack scalars
//! are handled.  Complex atoms (`Like`, `Regex`, `Contains`, `ContainsAny`,
//! `ContainsAll`, `Between`, `FtsMatch`, `ComputedCompare`, dynamic `In`)
//! return `None` immediately so the safe full-decode path runs instead.
//!
//! Restriction: only top-level and nested map fields (any depth) are resolved.
//! The raw-msgpack path supports the same depth as `resolve_field_ref` — it
//! iterates through nested maps one level at a time.

use std::cmp::Ordering;

use rmpv::Value as Rv;

use super::filter_node::{CompareOp, FilterNode};
use crate::query::filter::FilterValue;

/// Walk a msgpack `Rv::Map` to the field described by `path` (a slice of
/// interned `u64` keys).  Returns `None` if any segment is missing.
///
/// On-disk format: `InternerKey::serialize` calls `serialize_bytes`, so map
/// keys are encoded as msgpack `bin` containing the variable-width
/// little-endian ID (1/2/4/8 bytes per `InternerKey::new`).  The helper
/// re-encodes each path segment using the same scheme and compares as bytes.
/// A minimal `AsRef<[u8]>` wrapper for the variable-length key bytes.
struct KeyBuf([u8; 8], usize);
impl AsRef<[u8]> for KeyBuf {
    fn as_ref(&self) -> &[u8] {
        &self.0[..self.1]
    }
}

fn interned_key_bytes(id: u64) -> KeyBuf {
    let mut buf = [0u8; 8];
    let len = if id <= u8::MAX as u64 {
        buf[0] = id as u8;
        1
    } else if id <= u16::MAX as u64 {
        let b = (id as u16).to_le_bytes();
        buf[..2].copy_from_slice(&b);
        2
    } else if id <= u32::MAX as u64 {
        let b = (id as u32).to_le_bytes();
        buf[..4].copy_from_slice(&b);
        4
    } else {
        buf.copy_from_slice(&id.to_le_bytes());
        8
    };
    KeyBuf(buf, len)
}

fn find_field_bytes<'a>(root: &'a Rv, path: &[u64]) -> Option<&'a Rv> {
    let mut cur = root;
    for &id in path {
        let expected = interned_key_bytes(id);
        match cur {
            Rv::Map(entries) => {
                cur = entries.iter().find_map(|(k, v)| {
                    if let Rv::Binary(kb) = k {
                        if kb.as_slice() == expected.as_ref() {
                            return Some(v);
                        }
                    }
                    None
                })?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// Compare a msgpack scalar `Rv` against a `FilterValue` literal.
///
/// Returns `None` if either side is not a comparable scalar (array, map,
/// ext, non-literal `FilterValue`) — the caller must fall back to full decode.
fn compare_rv_to_filter(rv: &Rv, fv: &FilterValue) -> Option<Ordering> {
    match (rv, fv) {
        // Null
        (Rv::Nil, FilterValue::Null) => Some(Ordering::Equal),
        (_, FilterValue::Null) => None, // null vs non-null: not clearly ordered — fall back

        // Bool
        (Rv::Boolean(a), FilterValue::Bool(b)) => a.partial_cmp(b),

        // Int vs Int
        (Rv::Integer(n), FilterValue::Int(b)) => {
            let a = n.as_i64().or_else(|| n.as_u64().map(|u| u as i64))?;
            a.partial_cmp(b)
        }
        // Float vs Float
        (Rv::F64(a), FilterValue::Float(b)) => a.partial_cmp(b),
        (Rv::F32(a), FilterValue::Float(b)) => (*a as f64).partial_cmp(b),
        // Int vs Float (widening)
        (Rv::Integer(n), FilterValue::Float(b)) => {
            let a = n
                .as_i64()
                .map(|i| i as f64)
                .or_else(|| n.as_u64().map(|u| u as f64))?;
            a.partial_cmp(b)
        }
        // Float vs Int (widening)
        (Rv::F64(a), FilterValue::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Rv::F32(a), FilterValue::Int(b)) => (*a as f64).partial_cmp(&(*b as f64)),

        // Str vs Str
        (Rv::String(s), FilterValue::String(b)) => {
            let a = s.as_str()?;
            Some(a.cmp(b.as_str()))
        }

        // Binary vs Binary
        (Rv::Binary(a), FilterValue::Binary(b)) => Some(a.as_slice().cmp(b.as_slice())),

        // Mismatched / unsupported types → fall back to full decode
        _ => None,
    }
}

/// Apply a `CompareOp` to an `Ordering`, matching the semantics of
/// `filter_node.rs::matches` for the `Compare` variant.
#[inline]
fn apply_op(ord: Ordering, op: CompareOp) -> bool {
    match op {
        CompareOp::Eq => ord == Ordering::Equal,
        CompareOp::Ne => ord != Ordering::Equal,
        CompareOp::Gt => ord == Ordering::Greater,
        CompareOp::Gte => matches!(ord, Ordering::Greater | Ordering::Equal),
        CompareOp::Lt => ord == Ordering::Less,
        CompareOp::Lte => matches!(ord, Ordering::Less | Ordering::Equal),
    }
}

impl FilterNode {
    /// Try to evaluate this filter against raw msgpack bytes without decoding
    /// to `InnerValue`.
    ///
    /// # Returns
    /// - `Some(false)` — row is definitely rejected; skip full decode.
    /// - `Some(true)` — row passes; proceed to full decode + normal filter.
    /// - `None` — filter shape is unsupported here; fall back to full decode.
    ///
    /// **Precondition**: `bytes` must be a valid msgpack encoding of an
    /// `InnerValue::Map` with `u64` (interned) keys.  Bytes produced by
    /// any other codec return `None` (safe fall-through).
    pub fn matches_msgpack_bytes(&self, bytes: &[u8]) -> Option<bool> {
        // Parse the top-level msgpack value once; reuse the `Rv` tree for the
        // full filter walk.  On a parse error we return `None` — the row will
        // be decoded normally and the error surfaces there as usual.
        let rv = rmpv::decode::read_value(&mut &*bytes).ok()?;
        eval_node(self, &rv)
    }
}

/// Recursive filter evaluation on an already-parsed `Rv` tree.
fn eval_node(node: &FilterNode, root: &Rv) -> Option<bool> {
    match node {
        FilterNode::True => Some(true),
        FilterNode::False => Some(false),

        // ── Compare ──────────────────────────────────────────────────────────
        FilterNode::Compare {
            field_path,
            value,
            pre_resolved,
            op,
        } => {
            let rv_field = find_field_bytes(root, field_path)?;

            // Only handle literals pre-resolved at compile time or simple
            // FilterValue literals.  Dynamic variants (FieldRef, QueryRef,
            // Param, etc.) need the full InnerValue context → return None.
            let ord = if let Some(pre) = pre_resolved {
                // Fast path: pre-resolved literal available.
                // Compare rv_field against pre-resolved using the same
                // compare_rv_to_filter helper.
                use crate::query::filter::FilterValue as Fv;
                let fv_lit: Fv = inner_value_to_filter_value_lit(pre)?;
                compare_rv_to_filter(rv_field, &fv_lit)?
            } else {
                // FilterValue is dynamic or complex (FieldRef, QueryRef…) → fall back.
                match value {
                    FilterValue::Null
                    | FilterValue::Bool(_)
                    | FilterValue::Int(_)
                    | FilterValue::Float(_)
                    | FilterValue::String(_)
                    | FilterValue::Binary(_) => compare_rv_to_filter(rv_field, value)?,
                    _ => return None,
                }
            };
            Some(apply_op(ord, *op))
        }

        // ── Logical ──────────────────────────────────────────────────────────
        FilterNode::And(children) => {
            // Short-circuit: any None → None (fall back), any false → false.
            for child in children {
                match eval_node(child, root) {
                    Some(false) => return Some(false),
                    None => return None,
                    Some(true) => {}
                }
            }
            Some(true)
        }
        FilterNode::Or(children) => {
            for child in children {
                match eval_node(child, root) {
                    Some(true) => return Some(true),
                    None => return None,
                    Some(false) => {}
                }
            }
            Some(false)
        }
        FilterNode::Not(inner) => eval_node(inner, root).map(|b| !b),

        // ── Existence checks ─────────────────────────────────────────────────
        FilterNode::Exists { field_path } => Some(find_field_bytes(root, field_path).is_some()),
        FilterNode::NotExists { field_path } => Some(find_field_bytes(root, field_path).is_none()),
        FilterNode::IsNull { field_path } => {
            let rv = find_field_bytes(root, field_path);
            Some(matches!(rv, None | Some(Rv::Nil)))
        }
        FilterNode::IsNotNull { field_path } => {
            let rv = find_field_bytes(root, field_path);
            Some(!matches!(rv, None | Some(Rv::Nil)))
        }

        // ── Unsupported atoms — fall back to full decode ──────────────────────
        FilterNode::Like { .. }
        | FilterNode::Regex { .. }
        | FilterNode::Contains { .. }
        | FilterNode::ContainsAny { .. }
        | FilterNode::ContainsAll { .. }
        | FilterNode::Between { .. }
        | FilterNode::FtsMatch { .. }
        | FilterNode::ComputedCompare { .. }
        | FilterNode::In { .. } => None,
    }
}

/// Convert a pre-resolved `InnerValue` literal into a `FilterValue` literal
/// for the scalar comparison helper.  Non-scalar variants return `None`.
fn inner_value_to_filter_value_lit(
    v: &shamir_types::types::value::InnerValue,
) -> Option<FilterValue> {
    use shamir_types::types::value::InnerValue as Iv;
    match v {
        Iv::Null => Some(FilterValue::Null),
        Iv::Bool(b) => Some(FilterValue::Bool(*b)),
        Iv::Int(i) => Some(FilterValue::Int(*i)),
        Iv::F64(f) => Some(FilterValue::Float(*f)),
        Iv::Str(s) => Some(FilterValue::String(s.clone())),
        Iv::Bin(b) => Some(FilterValue::Binary(b.clone())),
        // Dec, Big, List, Set, Map — fall back to full decode
        _ => None,
    }
}
