use crate::index2::kind::{StemLanguage, TokenizerKind};

/// Parse a DSL tokenizer spec string into a [`TokenizerKind`].
///
/// DSL names:
///   - `None` / `"whitespace"` / unknown → `Whitespace`
///   - `"unicode"` → `Unicode`
///   - `"ngram"` → `Ngram { n: 3 }` (default trigram)
///   - `"ngram2"` .. `"ngram9"` → `Ngram { n: <digit> }`
///   - `"stemmed_<lang>"` → `Full { <lang>, stopwords=true, stem=true }`
///     (falls back to `Whitespace` if the language suffix is unknown)
pub(crate) fn fts_tokenizer_from_dsl(spec: Option<&str>) -> crate::index2::kind::TokenizerKind {
    match spec {
        Some("unicode") => TokenizerKind::Unicode,
        Some("ngram") => TokenizerKind::Ngram { n: 3 },
        Some(s) if s.starts_with("ngram") => {
            let digits = &s["ngram".len()..];
            let n: u8 = digits.parse().unwrap_or(3);
            TokenizerKind::Ngram { n: n.max(1) }
        }
        Some(s) if s.starts_with("stemmed_") => {
            let rest = &s["stemmed_".len()..];
            match StemLanguage::from_dsl(rest) {
                Some(lang) => TokenizerKind::Full {
                    language: lang,
                    stopwords: true,
                    stem: true,
                },
                None => TokenizerKind::Whitespace,
            }
        }
        _ => TokenizerKind::Whitespace,
    }
}
