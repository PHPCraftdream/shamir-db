//! `/validate` scalar category — predicate functions returning `Bool`.
//!
//! Functions registered (plain names, no folder prefix):
//! `is_email is_url is_uuid is_ipv4 is_ipv6 is_phone luhn in_range matches
//!  is_json is_empty len_between`.
//!
//! Conventions (mirroring [`crate::math`]):
//! - Every function returns a `Bool` via [`v_bool`]; these pair with the
//!   table validators feature (CHECK / BEFORE-write hooks).
//! - String-shaped predicates take a `Str` via [`arg_str`]; a non-`Str`
//!   argument yields `ScalarError("type_mismatch")`.
//! - Format checks (email/url/uuid/ip/phone) are validated with [`regex`]
//!   patterns compiled once per call site via [`std::sync::LazyLock`].
//! - `matches` compiles a user-supplied pattern; an invalid regex yields
//!   `ScalarError("bad_pattern")`.
//! - All functions are pure + deterministic (indexable).

use crate::registry::{arg_i64, arg_str, v_bool, FnEntry, ScalarError, ScalarRegistry};
use regex::Regex;
use shamir_types::types::value::QueryValue;
use std::sync::LazyLock;

static EMAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}$").unwrap());

static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^https?://[^\s/$.?#][^\s]*$").unwrap());

static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
        .unwrap()
});

static IPV4_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])(\.(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])){3}$",
    )
    .unwrap()
});

// Loose E.164: optional leading '+', then 7..=15 digits.
static PHONE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\+?[1-9][0-9]{6,14}$").unwrap());

/// Validate an IPv6 textual address (covers `::` compression and the
/// IPv4-mapped tail form) by deferring to the std-library parser.
fn is_ipv6_addr(s: &str) -> bool {
    s.parse::<std::net::Ipv6Addr>().is_ok()
}

/// Luhn checksum over the ASCII digits of `s` (rejects any non-digit, including
/// spaces/dashes — callers should strip separators first if desired). An empty
/// string is not a valid Luhn number.
fn luhn_valid(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for c in s.chars().rev() {
        let d = match c.to_digit(10) {
            Some(d) => d,
            None => return false,
        };
        let v = if double {
            let v = d * 2;
            if v > 9 {
                v - 9
            } else {
                v
            }
        } else {
            d
        };
        sum += v;
        double = !double;
    }
    sum.is_multiple_of(10)
}

/// Whether `s` is a syntactically valid JSON document. A self-contained
/// recursive-descent validator (the crate does not depend on `serde_json`):
/// it parses one top-level value, then requires only trailing whitespace.
fn is_json_str(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut p = JsonParser { b: bytes, i: 0 };
    p.skip_ws();
    if !p.value() {
        return false;
    }
    p.skip_ws();
    p.i == bytes.len()
}

struct JsonParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn skip_ws(&mut self) {
        while let Some(&c) = self.b.get(self.i) {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, lit: &[u8]) -> bool {
        if self.b[self.i..].starts_with(lit) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> bool {
        match self.b.get(self.i) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string(),
            Some(b't') => self.eat(b"true"),
            Some(b'f') => self.eat(b"false"),
            Some(b'n') => self.eat(b"null"),
            Some(c) if *c == b'-' || c.is_ascii_digit() => self.number(),
            _ => false,
        }
    }

    fn object(&mut self) -> bool {
        self.i += 1; // '{'
        self.skip_ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return true;
        }
        loop {
            self.skip_ws();
            if self.b.get(self.i) != Some(&b'"') || !self.string() {
                return false;
            }
            self.skip_ws();
            if self.b.get(self.i) != Some(&b':') {
                return false;
            }
            self.i += 1;
            self.skip_ws();
            if !self.value() {
                return false;
            }
            self.skip_ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return true;
                }
                _ => return false,
            }
        }
    }

    fn array(&mut self) -> bool {
        self.i += 1; // '['
        self.skip_ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return true;
        }
        loop {
            self.skip_ws();
            if !self.value() {
                return false;
            }
            self.skip_ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return true;
                }
                _ => return false,
            }
        }
    }

    fn string(&mut self) -> bool {
        self.i += 1; // opening '"'
        while let Some(&c) = self.b.get(self.i) {
            match c {
                b'"' => {
                    self.i += 1;
                    return true;
                }
                b'\\' => {
                    self.i += 1;
                    match self.b.get(self.i) {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.i += 1;
                        }
                        Some(b'u') => {
                            self.i += 1;
                            for _ in 0..4 {
                                match self.b.get(self.i) {
                                    Some(h) if h.is_ascii_hexdigit() => self.i += 1,
                                    _ => return false,
                                }
                            }
                        }
                        _ => return false,
                    }
                }
                // Control chars are not allowed unescaped.
                0x00..=0x1F => return false,
                _ => self.i += 1,
            }
        }
        false
    }

    fn number(&mut self) -> bool {
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        // int part
        match self.b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(c) if c.is_ascii_digit() => {
                while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                }
            }
            _ => return false,
        }
        // frac
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if !self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                return false;
            }
            while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        // exp
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                return false;
            }
            while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        self.i > start
    }
}

/// Whether a [`QueryValue`] counts as "empty": null, empty string, empty
/// list, empty map, or empty binary. Numbers and bools are never empty.
fn value_is_empty(v: &QueryValue) -> bool {
    match v {
        QueryValue::Null => true,
        QueryValue::Str(s) => s.is_empty(),
        QueryValue::List(l) => l.is_empty(),
        QueryValue::Bin(b) => b.is_empty(),
        QueryValue::Map(m) => m.is_empty(),
        _ => false,
    }
}

/// Register the `/validate` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "is_email",
        FnEntry::pure(
            |a| Ok(v_bool(EMAIL_RE.is_match(arg_str(a, 0)?))),
            1,
            Some(1),
        ),
    );
    reg.register(
        "is_url",
        FnEntry::pure(|a| Ok(v_bool(URL_RE.is_match(arg_str(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "is_uuid",
        FnEntry::pure(|a| Ok(v_bool(UUID_RE.is_match(arg_str(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "is_ipv4",
        FnEntry::pure(|a| Ok(v_bool(IPV4_RE.is_match(arg_str(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "is_ipv6",
        FnEntry::pure(|a| Ok(v_bool(is_ipv6_addr(arg_str(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "is_phone",
        FnEntry::pure(
            |a| Ok(v_bool(PHONE_RE.is_match(arg_str(a, 0)?))),
            1,
            Some(1),
        ),
    );
    reg.register(
        "luhn",
        FnEntry::pure(|a| Ok(v_bool(luhn_valid(arg_str(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "in_range",
        FnEntry::pure(
            |a| {
                let x = crate::registry::arg_dec(a, 0)?;
                let lo = crate::registry::arg_dec(a, 1)?;
                let hi = crate::registry::arg_dec(a, 2)?;
                if lo > hi {
                    return Err(ScalarError::new("bad_bounds"));
                }
                Ok(v_bool(x >= lo && x <= hi))
            },
            3,
            Some(3),
        ),
    );
    reg.register(
        "matches",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let pat = arg_str(a, 1)?;
                let re = Regex::new(pat).map_err(|_| ScalarError::new("bad_pattern"))?;
                Ok(v_bool(re.is_match(s)))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "is_json",
        FnEntry::pure(|a| Ok(v_bool(is_json_str(arg_str(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "is_empty",
        FnEntry::pure(
            |a| {
                let v = a.first().ok_or_else(|| ScalarError::new("missing_arg"))?;
                Ok(v_bool(value_is_empty(v)))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "len_between",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let lo = arg_i64(a, 1)?;
                let hi = arg_i64(a, 2)?;
                if lo < 0 || hi < 0 || lo > hi {
                    return Err(ScalarError::new("bad_bounds"));
                }
                let n = s.chars().count() as i64;
                Ok(v_bool(n >= lo && n <= hi))
            },
            3,
            Some(3),
        ),
    );
}

#[cfg(test)]
mod tests;
