//! `/encode` scalar category — binary/text encoding & escaping primitives.
//!
//! Functions registered (plain names, no folder prefix):
//! `base64_enc base64_dec base64url_enc base64url_dec hex_enc hex_dec
//!  base32_enc base32_dec url_encode url_decode html_escape json_escape`.
//!
//! Conventions (mirroring `math.rs`):
//! - Encoders accept bytes (`Bin`) **or** text (`Str`, taken as UTF-8 bytes)
//!   and return a `Str`. Decoders accept a `Str` and return `Bin`; malformed
//!   input yields `ScalarError("decode_failed")`.
//! - `url_encode` / `url_decode` operate on UTF-8 text and return a `Str`;
//!   `url_decode` rejects non-UTF-8 percent sequences with `"decode_failed"`.
//! - `html_escape` / `json_escape` are text→text escapers (return `Str`).
//! - Every function here is pure + deterministic.

use crate::registry::{arg_str, v_bytes, v_str, FnEntry, ScalarError, ScalarRegistry};
use base64::engine::general_purpose::{STANDARD, URL_SAFE};
use base64::Engine as _;
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use shamir_types::types::value::QueryValue;

/// Register the `/encode` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "base64_enc",
        FnEntry::pure(|a| Ok(v_str(STANDARD.encode(in_bytes(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "base64_dec",
        FnEntry::pure(
            |a| {
                STANDARD
                    .decode(arg_str(a, 0)?)
                    .map(v_bytes)
                    .map_err(|_| ScalarError::new("decode_failed"))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "base64url_enc",
        FnEntry::pure(|a| Ok(v_str(URL_SAFE.encode(in_bytes(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "base64url_dec",
        FnEntry::pure(
            |a| {
                URL_SAFE
                    .decode(arg_str(a, 0)?)
                    .map(v_bytes)
                    .map_err(|_| ScalarError::new("decode_failed"))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "hex_enc",
        FnEntry::pure(|a| Ok(v_str(hex::encode(in_bytes(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "hex_dec",
        FnEntry::pure(
            |a| {
                hex::decode(arg_str(a, 0)?)
                    .map(v_bytes)
                    .map_err(|_| ScalarError::new("decode_failed"))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "base32_enc",
        FnEntry::pure(
            |a| Ok(v_str(data_encoding::BASE32.encode(in_bytes(a, 0)?))),
            1,
            Some(1),
        ),
    );
    reg.register(
        "base32_dec",
        FnEntry::pure(
            |a| {
                data_encoding::BASE32
                    .decode(arg_str(a, 0)?.as_bytes())
                    .map(v_bytes)
                    .map_err(|_| ScalarError::new("decode_failed"))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "url_encode",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                Ok(v_str(utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "url_decode",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                percent_decode_str(s)
                    .decode_utf8()
                    .map(|cow| v_str(cow.into_owned()))
                    .map_err(|_| ScalarError::new("decode_failed"))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "html_escape",
        FnEntry::pure(|a| Ok(v_str(html_escape(arg_str(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "json_escape",
        FnEntry::pure(|a| Ok(v_str(json_escape(arg_str(a, 0)?))), 1, Some(1)),
    );
}

/// Extract encoder input as bytes: a `Bin` is used verbatim, a `Str` is taken as
/// its UTF-8 bytes. Any other variant is a `"type_mismatch"`.
fn in_bytes(args: &[QueryValue], i: usize) -> Result<&[u8], ScalarError> {
    match args.get(i).ok_or_else(|| ScalarError::new("missing_arg"))? {
        QueryValue::Bin(b) => Ok(b.as_slice()),
        QueryValue::Str(s) => Ok(s.as_bytes()),
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Escape the five XML/HTML metacharacters (`& < > " '`).
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a string for embedding inside a JSON string literal (no surrounding
/// quotes). Control characters below U+0020 use `\uXXXX`.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}
