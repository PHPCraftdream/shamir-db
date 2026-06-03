//! `/text` scalar category — Unicode normalisation, slugging, and string
//! similarity. Mirrors the [`crate::math`] reference: each function is an
//! [`FnEntry`] built with the `registry` value helpers, registered under a
//! plain name (no folder prefix), with machine-readable error codes only.
//!
//! Functions registered (plain names, no folder prefix):
//! `normalize_nfc normalize_nfkc slugify levenshtein jaro_winkler word_count
//!  truncate_ellipsis`.
//!
//! Conventions:
//! - Text transforms (`normalize_nfc`, `normalize_nfkc`, `slugify`,
//!   `truncate_ellipsis`) take a `Str` via [`arg_str`] and return a `Str`.
//! - Similarity ops (`levenshtein`, `jaro_winkler`) take two `Str` args;
//!   `levenshtein` returns an `Int` (edit distance), `jaro_winkler` a `Dec`
//!   in `[0, 1]` via [`v_f64`].
//! - `word_count` returns an `Int` (Unicode whitespace-split token count).
//! - All these functions are pure + deterministic, so `FnEntry::pure` applies.

use crate::registry::{arg_i64, arg_str, v_int, v_str, FnEntry, ScalarError, ScalarRegistry};
use unicode_normalization::UnicodeNormalization;

/// Register the `/text` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "normalize_nfc",
        FnEntry::pure(|a| Ok(v_str(arg_str(a, 0)?.nfc().collect())), 1, Some(1)),
    );
    reg.register(
        "normalize_nfkc",
        FnEntry::pure(|a| Ok(v_str(arg_str(a, 0)?.nfkc().collect())), 1, Some(1)),
    );
    reg.register(
        "slugify",
        FnEntry::pure(|a| Ok(v_str(slugify(arg_str(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "levenshtein",
        FnEntry::pure(
            |a| {
                let x = arg_str(a, 0)?;
                let y = arg_str(a, 1)?;
                let d = strsim::levenshtein(x, y);
                let d = i64::try_from(d).map_err(|_| ScalarError::new("out_of_range"))?;
                Ok(v_int(d))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "jaro_winkler",
        FnEntry::pure(
            |a| {
                let x = arg_str(a, 0)?;
                let y = arg_str(a, 1)?;
                crate::registry::v_f64(strsim::jaro_winkler(x, y))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "word_count",
        FnEntry::pure(
            |a| {
                let n = arg_str(a, 0)?.split_whitespace().count();
                let n = i64::try_from(n).map_err(|_| ScalarError::new("out_of_range"))?;
                Ok(v_int(n))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "truncate_ellipsis",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let max = arg_i64(a, 1)?;
                if max < 0 {
                    return Err(ScalarError::new("out_of_range"));
                }
                Ok(v_str(truncate_ellipsis(s, max as usize)))
            },
            2,
            Some(2),
        ),
    );
}

/// Lowercase ASCII slug: collapse any run of non-alphanumeric characters into a
/// single `-`, trim leading/trailing `-`. Non-ASCII characters are treated as
/// separators (callers should `normalize_nfkc` first if transliteration of
/// accents is desired).
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Truncate `s` to at most `max` Unicode scalar values, appending `…` when the
/// string is actually shortened. The ellipsis counts toward `max`: if `max == 0`
/// the result is empty; if `max` is too small to hold both content and the
/// ellipsis, as many leading characters as fit before the ellipsis are kept.
fn truncate_ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    // Reserve one char for the ellipsis.
    let keep = max - 1;
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests;
