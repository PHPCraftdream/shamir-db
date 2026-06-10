//! `/strings` scalar category — text manipulation built on the [`math`]
//! reference pattern.
//!
//! [`math`]: crate::math
//!
//! Functions registered (plain names, no folder prefix):
//! `lower upper trim ltrim rtrim length byte_length substring concat replace
//!  split starts_with ends_with contains index_of repeat reverse pad_left
//!  pad_right` plus the regex family `is_reg_match reg_query reg_query_all
//!  reg_captures reg_replace reg_split reg_count reg_find_index`.
//!
//! Conventions (mirroring `math.rs`):
//! - String args via [`arg_str`], integer args via [`arg_i64`].
//! - `length` is a Unicode **scalar** count (`chars().count()`), `byte_length`
//!   is the UTF-8 byte length; `substring`/`index_of` index in `char`s.
//! - Errors are machine codes: `type_mismatch`, `out_of_range`, `bad_regex`,
//!   `no_match`, `empty`.
//! - Regexes are compiled once and cached by pattern (Rust `regex` is
//!   ReDoS-safe, so no input-size guard is needed).

use crate::registry::{
    arg_i64, arg_str, v_bool, v_int, v_list, v_str, FnEntry, ScalarError, ScalarRegistry,
    ScalarResult,
};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Register the `/strings` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "lower",
        FnEntry::pure(|a| Ok(v_str(arg_str(a, 0)?.to_lowercase())), 1, Some(1)),
    );
    reg.register(
        "upper",
        FnEntry::pure(|a| Ok(v_str(arg_str(a, 0)?.to_uppercase())), 1, Some(1)),
    );
    reg.register(
        "trim",
        FnEntry::pure(|a| Ok(v_str(arg_str(a, 0)?.trim().to_string())), 1, Some(1)),
    );
    reg.register(
        "ltrim",
        FnEntry::pure(
            |a| Ok(v_str(arg_str(a, 0)?.trim_start().to_string())),
            1,
            Some(1),
        ),
    );
    reg.register(
        "rtrim",
        FnEntry::pure(
            |a| Ok(v_str(arg_str(a, 0)?.trim_end().to_string())),
            1,
            Some(1),
        ),
    );
    reg.register(
        "length",
        FnEntry::pure(
            |a| Ok(v_int(arg_str(a, 0)?.chars().count() as i64)),
            1,
            Some(1),
        ),
    );
    reg.register(
        "byte_length",
        FnEntry::pure(|a| Ok(v_int(arg_str(a, 0)?.len() as i64)), 1, Some(1)),
    );
    reg.register(
        "substring",
        FnEntry::pure(
            |a| {
                // substring(s, start, len) — start is a 0-based char offset, len
                // a char count. Out-of-range start/negative args -> out_of_range.
                let s = arg_str(a, 0)?;
                let start = arg_i64(a, 1)?;
                let len = arg_i64(a, 2)?;
                if start < 0 || len < 0 {
                    return Err(ScalarError::new("out_of_range"));
                }
                let out: String = s.chars().skip(start as usize).take(len as usize).collect();
                Ok(v_str(out))
            },
            3,
            Some(3),
        ),
    );
    reg.register(
        "concat",
        FnEntry::pure(
            |a| {
                let mut out = String::new();
                for i in 0..a.len() {
                    out.push_str(arg_str(a, i)?);
                }
                Ok(v_str(out))
            },
            1,
            None,
        ),
    );
    reg.register(
        "replace",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let from = arg_str(a, 1)?;
                let to = arg_str(a, 2)?;
                Ok(v_str(s.replace(from, to)))
            },
            3,
            Some(3),
        ),
    );
    reg.register(
        "split",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let sep = arg_str(a, 1)?;
                if sep.is_empty() {
                    return Err(ScalarError::new("empty"));
                }
                let items = s.split(sep).map(|p| v_str(p.to_string())).collect();
                Ok(v_list(items))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "starts_with",
        FnEntry::pure(
            |a| Ok(v_bool(arg_str(a, 0)?.starts_with(arg_str(a, 1)?))),
            2,
            Some(2),
        ),
    );
    reg.register(
        "ends_with",
        FnEntry::pure(
            |a| Ok(v_bool(arg_str(a, 0)?.ends_with(arg_str(a, 1)?))),
            2,
            Some(2),
        ),
    );
    reg.register(
        "contains",
        FnEntry::pure(
            |a| Ok(v_bool(arg_str(a, 0)?.contains(arg_str(a, 1)?))),
            2,
            Some(2),
        ),
    );
    reg.register(
        "index_of",
        FnEntry::pure(
            |a| {
                // 0-based char index of the first occurrence of needle, or -1.
                let s = arg_str(a, 0)?;
                let needle = arg_str(a, 1)?;
                let idx = match s.find(needle) {
                    Some(byte_pos) => s[..byte_pos].chars().count() as i64,
                    None => -1,
                };
                Ok(v_int(idx))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "repeat",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let n = arg_i64(a, 1)?;
                if n < 0 {
                    return Err(ScalarError::new("out_of_range"));
                }
                Ok(v_str(s.repeat(n as usize)))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "reverse",
        FnEntry::pure(
            |a| Ok(v_str(arg_str(a, 0)?.chars().rev().collect())),
            1,
            Some(1),
        ),
    );
    reg.register("pad_left", FnEntry::pure(|a| pad(a, Pad::Left), 3, Some(3)));
    reg.register(
        "pad_right",
        FnEntry::pure(|a| pad(a, Pad::Right), 3, Some(3)),
    );

    // ---- regex family --------------------------------------------------
    reg.register(
        "is_reg_match",
        FnEntry::pure(
            |a| {
                let re = compile(arg_str(a, 1)?)?;
                Ok(v_bool(re.is_match(arg_str(a, 0)?)))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "reg_query",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let re = compile(arg_str(a, 1)?)?;
                match re.find(s) {
                    Some(m) => Ok(v_str(m.as_str().to_string())),
                    None => Err(ScalarError::new("no_match")),
                }
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "reg_query_all",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let re = compile(arg_str(a, 1)?)?;
                let items = re
                    .find_iter(s)
                    .map(|m| v_str(m.as_str().to_string()))
                    .collect();
                Ok(v_list(items))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "reg_captures",
        FnEntry::pure(
            |a| {
                // Capture groups of the first match (group 0 = whole match).
                // A non-participating group yields "". No match -> no_match.
                let s = arg_str(a, 0)?;
                let re = compile(arg_str(a, 1)?)?;
                match re.captures(s) {
                    Some(caps) => {
                        let items = caps
                            .iter()
                            .map(|g| v_str(g.map(|m| m.as_str()).unwrap_or("").to_string()))
                            .collect();
                        Ok(v_list(items))
                    }
                    None => Err(ScalarError::new("no_match")),
                }
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "reg_replace",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let re = compile(arg_str(a, 1)?)?;
                let repl = arg_str(a, 2)?;
                Ok(v_str(re.replace_all(s, repl).into_owned()))
            },
            3,
            Some(3),
        ),
    );
    reg.register(
        "reg_split",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let re = compile(arg_str(a, 1)?)?;
                let items = re.split(s).map(|p| v_str(p.to_string())).collect();
                Ok(v_list(items))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "reg_count",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let re = compile(arg_str(a, 1)?)?;
                Ok(v_int(re.find_iter(s).count() as i64))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "reg_find_index",
        FnEntry::pure(
            |a| {
                // 0-based char index of the first match start, or -1.
                let s = arg_str(a, 0)?;
                let re = compile(arg_str(a, 1)?)?;
                let idx = match re.find(s) {
                    Some(m) => s[..m.start()].chars().count() as i64,
                    None => -1,
                };
                Ok(v_int(idx))
            },
            2,
            Some(2),
        ),
    );
}

enum Pad {
    Left,
    Right,
}

/// `pad_left`/`pad_right(s, len, ch)` — pad `s` with the (single-char) `ch` to a
/// target **char** width of `len`. `len < 0` -> `out_of_range`; `ch` that is not
/// exactly one char -> `bad_pad`. A string already at/over width is returned
/// unchanged (no truncation).
fn pad(args: &[shamir_types::types::value::InnerValue], side: Pad) -> ScalarResult {
    let s = arg_str(args, 0)?;
    let len = arg_i64(args, 1)?;
    if len < 0 {
        return Err(ScalarError::new("out_of_range"));
    }
    let ch_str = arg_str(args, 2)?;
    let mut chs = ch_str.chars();
    let ch = match (chs.next(), chs.next()) {
        (Some(c), None) => c,
        _ => return Err(ScalarError::new("bad_pad")),
    };
    let cur = s.chars().count();
    let target = len as usize;
    if cur >= target {
        return Ok(v_str(s.to_string()));
    }
    let fill: String = std::iter::repeat_n(ch, target - cur).collect();
    let out = match side {
        Pad::Left => format!("{fill}{s}"),
        Pad::Right => format!("{s}{fill}"),
    };
    Ok(v_str(out))
}

/// Compile `pat`, caching by pattern string. Rust's `regex` engine has linear
/// time guarantees, so a shared cache is ReDoS-safe. Invalid patterns ->
/// `ScalarError("bad_regex")`.
fn compile(pat: &str) -> Result<Regex, ScalarError> {
    static CACHE: OnceLock<Mutex<HashMap<String, Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    // Lock is poison-tolerant: a panic-while-compiling cannot corrupt the map
    // (Regex compilation does not mutate it under the guard), so recover the
    // inner map either way.
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(re) = guard.get(pat) {
        return Ok(re.clone());
    }
    let re = Regex::new(pat).map_err(|_| ScalarError::new("bad_regex"))?;
    // Bound the cache to avoid unbounded growth from adversarial pattern churn.
    if guard.len() >= 256 {
        guard.clear();
    }
    guard.insert(pat.to_string(), re.clone());
    Ok(re)
}
