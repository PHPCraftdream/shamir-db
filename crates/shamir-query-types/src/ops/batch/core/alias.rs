//! Alias-reference normalization shared by the planner and (eventually)
//! query builders.
//!
//! `extract_base_alias` strips the leading `@` reference marker and any
//! trailing path (`[0]`, `.field`, or a mix) from a `$query`/`after`
//! reference string, leaving the bare alias used as a key in the batch's
//! `queries` map.

/// Extract the base alias from a reference string like `"@users[0].id"` or
/// bare `"users"` → `"users"`.
///
/// The leading `@` is the explicit reference marker (per spec) and is
/// stripped before lookup against the queries map (whose keys never carry
/// `@`). Both forms are accepted on input — `@user` and bare `user` map to
/// the same alias — but the canonical, documented form is **with** the `@`.
pub fn extract_base_alias(s: &str) -> String {
    let s = s.strip_prefix('@').unwrap_or(s);
    s.find(['[', '.'])
        .map(|pos| s[..pos].to_string())
        .unwrap_or_else(|| s.to_string())
}

/// If `s` carries a path tail (`[`/`.` after the base alias), return
/// `(base_alias, path_tail)`. Used to detect `after` entries that name a
/// specific value path — a form only meaningful for `$query` (which resolves
/// to that value) and never for `after` (which is alias-only ordering, no
/// value resolution happens).
pub(crate) fn split_path_tail(s: &str) -> Option<(String, String)> {
    let stripped = s.strip_prefix('@').unwrap_or(s);
    stripped
        .find(['[', '.'])
        .map(|pos| (stripped[..pos].to_string(), stripped[pos..].to_string()))
}
