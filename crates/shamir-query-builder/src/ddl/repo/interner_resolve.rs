//! Pure (no-I/O) field-name → interner-id resolution helper for client-side
//! planning.
//!
//! The builder itself cannot touch the server (it has no transport), so this
//! module takes a resolver closure (typically `|n| client.resolve_field(db,
//! repo, n)`) and maps a field-path string to an `Option<u64>`. It lives in the
//! builder crate so that planning code can pre-resolve field paths without
//! pulling the client crate (which would be a cycle).

/// A field-name → interner-id resolver the builder can call without I/O.
///
/// Implementations are supplied by the caller (e.g. the client wraps its
/// `FieldMap` cache). The builder never touches the server through this trait —
/// it is a pure lookup.
pub trait FieldResolver {
    /// Resolve a field name to its interner id.
    ///
    /// §9.4: `name` is an opaque STRING. The literal `"42"` resolves to the
    /// field named "42"; it is NEVER parsed to the integer 42.
    fn resolve(&self, name: &str) -> Option<u64>;
}

/// Blanket impl: any closure `Fn(&str) -> Option<u64>` is a resolver.
impl<F> FieldResolver for F
where
    F: Fn(&str) -> Option<u64>,
{
    fn resolve(&self, name: &str) -> Option<u64> {
        (self)(name)
    }
}

/// Resolve a single field-path name → id via `resolver`.
///
/// A "field path" here is a dotted name like `"profile.age"`; the WHOLE string
/// is resolved as one field name (the interner keys on the literal string, not
/// on path segments). Returns `None` when the resolver has no mapping.
///
/// Pure: no allocation beyond the lookup, no I/O.
pub fn resolve_field_path<R: FieldResolver>(resolver: &R, name: &str) -> Option<u64> {
    // §9.4: `name` is passed through verbatim — never parsed, never split, never
    // numeric-coerced. The interner treats it as an opaque string key.
    resolver.resolve(name)
}

/// Resolve a batch of field-path names → ids. Unknown names are dropped from
/// the result (the caller decides whether to touch them).
///
/// Pure: O(N) resolver calls, no I/O.
pub fn resolve_field_paths<'a, R: FieldResolver, I>(resolver: &R, names: I) -> Vec<(&'a str, u64)>
where
    I: IntoIterator<Item = &'a str>,
{
    names
        .into_iter()
        .filter_map(|n| resolver.resolve(n).map(|id| (n, id)))
        .collect()
}
