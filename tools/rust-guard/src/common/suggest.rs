//! Rendering of `--suggest-delta` TOML files: the `[[moved]]` pairings
//! the gates could not honour because the delta does not declare them.
//! Purely diagnostic — the file is a starting point for the author, the
//! gate's findings and exit code are untouched.

use std::collections::BTreeSet;

/// TOML basic-string escaping: backslash, quote, the short escapes for
/// BS/HT/LF/FF/CR, and every other control character (incl. DEL) as
/// `\uXXXX`. Rust's `{:?}` is NOT a TOML serializer (`\u{..}` is invalid
/// TOML).
fn escape_toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// One undeclared move: base side (`path`, `item`) paired with the head
/// side the comparison found for it (`to_path`, `to_item`).
pub(crate) struct MoveSuggestion {
    pub path: String,
    pub item: String,
    pub to_path: String,
    pub to_item: String,
}

/// Writes the suggestion file. A no-op (`Ok`, nothing written, no note)
/// when `moves` is empty; otherwise entries are sorted by (path, item),
/// deduplicated, rendered as TOML and written to `out_path`, with one
/// summary line on stderr.
pub(crate) fn write(out_path: &str, base: &str, moves: &[MoveSuggestion]) -> Result<(), String> {
    if moves.is_empty() {
        return Ok(());
    }
    // BTreeSet gives both the deterministic order (base path, then base
    // item) and exact-duplicate removal in one step.
    let sorted: BTreeSet<(&String, &String, &String, &String)> = moves
        .iter()
        .map(|m| (&m.path, &m.item, &m.to_path, &m.to_item))
        .collect();
    let mut text = String::new();
    text.push_str("base = \"");
    text.push_str(&escape_toml_string(base));
    text.push_str("\"\n");
    for (path, item, to_path, to_item) in &sorted {
        text.push_str(&format!(
            "\n[[moved]]\npath = \"{}\"\nitem = \"{}\"\nto_path = \"{}\"\nto_item = \"{}\"\nreason = \"relocated by the layout sweep\"\nowner = \"@layout-sweep\"\n",
            escape_toml_string(path),
            escape_toml_string(item),
            escape_toml_string(to_path),
            escape_toml_string(to_item)
        ));
    }
    std::fs::write(out_path, text)
        .map_err(|e| format!("cannot write suggestion file {out_path}: {e}"))?;
    eprintln!(
        "rust-guard: note: {} undeclared move(s) written to {out_path}",
        sorted.len()
    );
    Ok(())
}
