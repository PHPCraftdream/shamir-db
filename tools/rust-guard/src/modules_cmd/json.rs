//! Minimal JSON reader for `cargo metadata --no-deps --format-version 1`.
//!
//! The crate deliberately avoids a full JSON dependency; this recursive-
//! descent parser covers the subset cargo emits (objects, arrays, strings
//! with standard escapes, numbers, booleans, null).

use std::collections::BTreeSet;

/// A parsed JSON value. Booleans and null (unused by this reader) are
/// kept as their literal text.
#[derive(Debug, Clone)]
pub enum Json {
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Object field lookup.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// String value, if this node is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Array slice, if this node is an array.
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// Collects string elements of an array into a set.
    pub fn str_set(&self) -> BTreeSet<String> {
        self.as_arr()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.as_str())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Parses `text`; errors name the byte offset.
pub fn parse(text: &str) -> Result<Json, String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut pos = 0usize;
    let value = parse_value(&bytes, &mut pos)?;
    skip_ws(&bytes, &mut pos);
    if pos != bytes.len() {
        return Err(format!("trailing JSON content at char {pos}"));
    }
    Ok(value)
}

fn skip_ws(b: &[char], pos: &mut usize) {
    while *pos < b.len() && b[*pos].is_whitespace() {
        *pos += 1;
    }
}

fn parse_value(b: &[char], pos: &mut usize) -> Result<Json, String> {
    skip_ws(b, pos);
    match b.get(*pos) {
        None => Err("unexpected end of JSON".into()),
        Some('{') => parse_obj(b, pos),
        Some('[') => parse_arr(b, pos),
        Some('"') => Ok(Json::Str(parse_string(b, pos)?)),
        Some('t') => lit(b, pos, "true"),
        Some('f') => lit(b, pos, "false"),
        Some('n') => lit(b, pos, "null"),
        Some(_) => parse_num(b, pos),
    }
}

fn lit(b: &[char], pos: &mut usize, word: &str) -> Result<Json, String> {
    if b[*pos..].starts_with(word.chars().collect::<Vec<_>>().as_slice()) {
        *pos += word.len();
        Ok(Json::Str(word.to_owned()))
    } else {
        Err(format!("bad literal at char {pos}"))
    }
}

fn parse_obj(b: &[char], pos: &mut usize) -> Result<Json, String> {
    *pos += 1; // '{'
    let mut pairs = Vec::new();
    skip_ws(b, pos);
    if b.get(*pos) == Some(&'}') {
        *pos += 1;
        return Ok(Json::Obj(pairs));
    }
    loop {
        skip_ws(b, pos);
        if b.get(*pos) != Some(&'"') {
            return Err(format!("expected object key at char {pos}"));
        }
        let key = parse_string(b, pos)?;
        skip_ws(b, pos);
        if b.get(*pos) != Some(&':') {
            return Err(format!("expected ':' at char {pos}"));
        }
        *pos += 1;
        let value = parse_value(b, pos)?;
        pairs.push((key, value));
        skip_ws(b, pos);
        match b.get(*pos) {
            Some(',') => *pos += 1,
            Some('}') => {
                *pos += 1;
                return Ok(Json::Obj(pairs));
            }
            _ => return Err(format!("expected ',' or '}}' at char {pos}")),
        }
    }
}

fn parse_arr(b: &[char], pos: &mut usize) -> Result<Json, String> {
    *pos += 1; // '['
    let mut items = Vec::new();
    skip_ws(b, pos);
    if b.get(*pos) == Some(&']') {
        *pos += 1;
        return Ok(Json::Arr(items));
    }
    loop {
        items.push(parse_value(b, pos)?);
        skip_ws(b, pos);
        match b.get(*pos) {
            Some(',') => *pos += 1,
            Some(']') => {
                *pos += 1;
                return Ok(Json::Arr(items));
            }
            _ => return Err(format!("expected ',' or ']' at char {pos}")),
        }
    }
}

fn parse_string(b: &[char], pos: &mut usize) -> Result<String, String> {
    *pos += 1; // opening '"'
    let mut out = String::new();
    loop {
        match b.get(*pos) {
            None => return Err("unterminated string".into()),
            Some('"') => {
                *pos += 1;
                return Ok(out);
            }
            Some('\\') => {
                *pos += 1;
                let esc = *b.get(*pos).ok_or("unterminated escape")?;
                *pos += 1;
                match esc {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let hex: String = b[*pos..*pos + 4].iter().collect();
                        *pos += 4;
                        let cp = u32::from_str_radix(&hex, 16)
                            .map_err(|_| format!("bad \\u escape at char {pos}"))?;
                        out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                    }
                    other => out.push(other),
                }
            }
            Some(&c) => {
                out.push(c);
                *pos += 1;
            }
        }
    }
}

fn parse_num(b: &[char], pos: &mut usize) -> Result<Json, String> {
    let start = *pos;
    while *pos < b.len() && (b[*pos].is_ascii_digit() || "+-.eE".contains(b[*pos])) {
        *pos += 1;
    }
    let text: String = b[start..*pos].iter().collect();
    text.parse::<f64>()
        .map(|_| Json::Str(text))
        .map_err(|_| format!("bad number at char {start}"))
}
