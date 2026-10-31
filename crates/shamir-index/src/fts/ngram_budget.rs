//! Per-value limits for fallible n-gram tokenization.

use std::borrow::Cow;

use crate::backends::backend::IndexError;

pub(crate) const MAX_NGRAM_TOKENS_PER_VALUE: u64 = 1_048_576;
pub(crate) const MAX_NGRAM_BYTES_PER_VALUE: u64 = 16 * 1024 * 1024;

/// Boundary pairs for each full n-character window, without a boundary buffer.
fn windows(word: &str, n: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
    let starts = word.char_indices().map(|(i, _)| i);
    let ends = word
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(word.len()))
        .skip(n);
    starts.zip(ends)
}

pub(crate) fn visit(text: &str, n: usize, mut visitor: impl FnMut(&str)) {
    for word in text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let lowered = word.to_lowercase();
        if lowered.chars().count() <= n {
            visitor(&lowered);
        } else {
            for (start, end) in windows(&lowered, n) {
                visitor(&lowered[start..end]);
            }
        }
    }
}

pub(crate) fn emit<'a>(word: &str, n: usize, out: &mut Vec<Cow<'a, str>>) {
    let lowered = word.to_lowercase();
    emit_lowered(lowered, n, out);
}

fn emit_lowered<'a>(lowered: String, n: usize, out: &mut Vec<Cow<'a, str>>) {
    if lowered.chars().count() <= n {
        out.push(Cow::Owned(lowered));
    } else {
        for (start, end) in windows(&lowered, n) {
            out.push(Cow::Owned(lowered[start..end].to_owned()));
        }
    }
}

pub(crate) fn try_tokenize(text: &str, n: usize) -> Result<Vec<Cow<'_, str>>, IndexError> {
    let mut out = Vec::new();
    let mut tokens = 0_u64;
    let mut bytes = 0_u64;
    for word in text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let lowered = word.to_lowercase();
        let chars = lowered.chars().count();
        let count = if chars <= n { 1 } else { chars - n + 1 };
        let word_bytes = if chars <= n {
            lowered.len() as u64
        } else {
            windows(&lowered, n).fold(0_u64, |sum, (start, end)| {
                sum.saturating_add((end - start) as u64)
            })
        };
        let next_tokens = tokens.saturating_add(count as u64);
        let next_bytes = bytes.saturating_add(word_bytes);
        if next_tokens > MAX_NGRAM_TOKENS_PER_VALUE {
            return Err(IndexError::TokenBudgetExceeded {
                limit_kind: "tokens",
                limit: MAX_NGRAM_TOKENS_PER_VALUE,
                observed: next_tokens,
            });
        }
        if next_bytes > MAX_NGRAM_BYTES_PER_VALUE {
            return Err(IndexError::TokenBudgetExceeded {
                limit_kind: "bytes",
                limit: MAX_NGRAM_BYTES_PER_VALUE,
                observed: next_bytes,
            });
        }
        tokens = next_tokens;
        bytes = next_bytes;
        emit_lowered(lowered, n, &mut out);
    }
    Ok(out)
}
