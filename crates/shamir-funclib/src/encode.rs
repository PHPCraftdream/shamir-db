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
use shamir_types::types::value::InnerValue;

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
fn in_bytes(args: &[InnerValue], i: usize) -> Result<&[u8], ScalarError> {
    match args.get(i).ok_or_else(|| ScalarError::new("missing_arg"))? {
        InnerValue::Bin(b) => Ok(b.as_slice()),
        InnerValue::Str(s) => Ok(s.as_bytes()),
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

#[cfg(test)]
mod tests {
    use crate::encode;
    use crate::registry::ScalarRegistry;
    use shamir_types::types::value::InnerValue;

    fn reg() -> ScalarRegistry {
        let mut r = ScalarRegistry::new();
        encode::register(&mut r);
        r
    }

    fn s(v: &str) -> InnerValue {
        InnerValue::Str(v.into())
    }

    fn b(v: &[u8]) -> InnerValue {
        InnerValue::Bin(v.to_vec())
    }

    #[test]
    fn base64_roundtrip_and_error() {
        let r = reg();
        assert_eq!(r.call("base64_enc", &[s("foo")]).unwrap(), s("Zm9v"));
        assert_eq!(r.call("base64_dec", &[s("Zm9v")]).unwrap(), b(b"foo"));
        // error: invalid base64 alphabet
        assert_eq!(
            r.call("base64_dec", &[s("!!!!")]).unwrap_err().code,
            "decode_failed"
        );
    }

    #[test]
    fn base64url_roundtrip_and_error() {
        let r = reg();
        // 0xfb 0xff encodes to "-_" in url-safe (vs "+/" standard).
        let enc = r.call("base64url_enc", &[b(&[0xfb, 0xff])]).unwrap();
        assert_eq!(enc, s("-_8="));
        assert_eq!(r.call("base64url_dec", &[enc]).unwrap(), b(&[0xfb, 0xff]));
        assert_eq!(
            r.call("base64url_dec", &[s("****")]).unwrap_err().code,
            "decode_failed"
        );
    }

    #[test]
    fn hex_roundtrip_and_error() {
        let r = reg();
        assert_eq!(r.call("hex_enc", &[b(&[0xde, 0xad])]).unwrap(), s("dead"));
        assert_eq!(r.call("hex_dec", &[s("dead")]).unwrap(), b(&[0xde, 0xad]));
        // error: odd length / non-hex digit
        assert_eq!(
            r.call("hex_dec", &[s("xyz")]).unwrap_err().code,
            "decode_failed"
        );
    }

    #[test]
    fn base32_roundtrip_and_error() {
        let r = reg();
        assert_eq!(r.call("base32_enc", &[s("foo")]).unwrap(), s("MZXW6==="));
        assert_eq!(r.call("base32_dec", &[s("MZXW6===")]).unwrap(), b(b"foo"));
        // error: invalid base32 symbol
        assert_eq!(
            r.call("base32_dec", &[s("0189")]).unwrap_err().code,
            "decode_failed"
        );
    }

    #[test]
    fn url_encode_decode_and_error() {
        let r = reg();
        assert_eq!(r.call("url_encode", &[s("a b/c")]).unwrap(), s("a%20b%2Fc"));
        assert_eq!(r.call("url_decode", &[s("a%20b%2Fc")]).unwrap(), s("a b/c"));
        // error: percent sequence decodes to invalid UTF-8 (lone 0xFF byte)
        assert_eq!(
            r.call("url_decode", &[s("%FF")]).unwrap_err().code,
            "decode_failed"
        );
    }

    #[test]
    fn html_escape_metachars() {
        let r = reg();
        assert_eq!(
            r.call("html_escape", &[s("<a href=\"x\">&'</a>")]).unwrap(),
            s("&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;")
        );
        // edge: a plain string is unchanged
        assert_eq!(r.call("html_escape", &[s("plain")]).unwrap(), s("plain"));
    }

    #[test]
    fn json_escape_specials() {
        let r = reg();
        assert_eq!(
            r.call("json_escape", &[s("a\"\\\n\t")]).unwrap(),
            s("a\\\"\\\\\\n\\t")
        );
        // edge: control char below 0x20 uses \u escape
        assert_eq!(r.call("json_escape", &[s("\u{01}")]).unwrap(), s("\\u0001"));
    }

    #[test]
    fn encoders_reject_non_text_non_bin() {
        let r = reg();
        assert_eq!(
            r.call("base64_enc", &[InnerValue::Int(7)])
                .unwrap_err()
                .code,
            "type_mismatch"
        );
        assert_eq!(
            r.call("hex_enc", &[InnerValue::Int(7)]).unwrap_err().code,
            "type_mismatch"
        );
    }
}
