use regex::Regex;

/// Convert a SQL LIKE pattern to a regex pattern.
/// `%` matches any sequence of characters, `_` matches a single character.
/// All other regex meta-characters are escaped.
pub(super) fn like_pattern_to_regex(pattern: &str, case_insensitive: bool) -> Option<Regex> {
    let mut regex_str = String::with_capacity(pattern.len() + 4);
    if case_insensitive {
        regex_str.push_str("(?i)");
    }
    regex_str.push('^');
    for ch in pattern.chars() {
        match ch {
            '%' => regex_str.push_str(".*"),
            '_' => regex_str.push('.'),
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '|' | '^' | '$' => {
                regex_str.push('\\');
                regex_str.push(ch);
            }
            _ => regex_str.push(ch),
        }
    }
    regex_str.push('$');
    Regex::new(&regex_str).ok()
}

/// Compare `word` (raw, possibly mixed-case) against the pre-lowercased
/// query token at `idx`. Avoids allocating a fresh `String` per word on
/// the ASCII-already-lowercase fast path (the common case for English
/// corpora). Falls back to a full Unicode `to_lowercase()` allocation
/// only when the word actually contains uppercase characters.
#[inline]
pub(super) fn fts_word_eq_token(word: &str, token: &str) -> bool {
    if word.is_ascii() {
        // ASCII fast path: bytewise compare with ASCII case folding.
        word.eq_ignore_ascii_case(token)
    } else {
        // Non-ASCII: must apply full Unicode lowercasing to preserve the
        // semantics of the previous `text.to_lowercase()` whole-string
        // pass. `str::to_lowercase` is per-char, so applying it to each
        // whitespace-split word produces the same bytes as folding the
        // whole string and then splitting.
        word.to_lowercase() == token
    }
}

/// AND-mode word probe (bitmask variant, up to 64 query tokens).
/// Returns whether the word matched anything; updates `seen` in place.
#[inline]
pub(super) fn fts_word_matches(word: &str, query_tokens: &[String], seen: &mut u64) -> bool {
    let mut hit = false;
    for (i, t) in query_tokens.iter().enumerate() {
        let bit = 1u64 << i;
        if (*seen & bit) == 0 && fts_word_eq_token(word, t.as_str()) {
            *seen |= bit;
            hit = true;
        }
    }
    hit
}

/// AND-mode word probe (Vec<bool> variant, > 64 query tokens — rare).
#[inline]
pub(super) fn fts_word_matches_vec(
    word: &str,
    query_tokens: &[String],
    seen: &mut [bool],
    remaining: &mut usize,
) -> bool {
    let mut hit = false;
    for (i, t) in query_tokens.iter().enumerate() {
        if !seen[i] && fts_word_eq_token(word, t.as_str()) {
            seen[i] = true;
            *remaining -= 1;
            hit = true;
        }
    }
    hit
}

/// OR-mode word probe: any-hit short-circuit.
#[inline]
pub(super) fn fts_word_matches_or(word: &str, query_tokens: &[String]) -> bool {
    query_tokens
        .iter()
        .any(|t| fts_word_eq_token(word, t.as_str()))
}
