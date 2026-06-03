//! Minimal tokenizers for FTS indexes.
//!
//! Zero-copy on ASCII paths (Cow::Borrowed), allocates only for
//! case-folding non-ASCII. No heavy deps — just std + manual
//! iteration.

use std::borrow::Cow;

use crate::index2::kind::StemLanguage;

pub trait Tokenizer: Send + Sync {
    fn tokenize<'a>(&self, text: &'a str) -> Vec<Cow<'a, str>>;
}

/// Split on whitespace, lowercase each token.
pub struct WhitespaceTokenizer;

impl Tokenizer for WhitespaceTokenizer {
    fn tokenize<'a>(&self, text: &'a str) -> Vec<Cow<'a, str>> {
        text.split_whitespace()
            .map(|word| {
                if word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || !b.is_ascii_alphabetic())
                {
                    Cow::Borrowed(word)
                } else {
                    Cow::Owned(word.to_lowercase())
                }
            })
            .collect()
    }
}

/// Split on non-alphanumeric boundaries (unicode-aware), lowercase.
pub struct UnicodeTokenizer;

impl Tokenizer for UnicodeTokenizer {
    fn tokenize<'a>(&self, text: &'a str) -> Vec<Cow<'a, str>> {
        let mut tokens = Vec::new();
        let mut start = None;
        for (i, ch) in text.char_indices() {
            if ch.is_alphanumeric() {
                if start.is_none() {
                    start = Some(i);
                }
            } else if let Some(s) = start.take() {
                let word = &text[s..i];
                tokens.push(lowercase_cow(word));
            }
        }
        if let Some(s) = start {
            tokens.push(lowercase_cow(&text[s..]));
        }
        tokens
    }
}

fn lowercase_cow(word: &str) -> Cow<'_, str> {
    if word.chars().all(|c| !c.is_uppercase()) {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(word.to_lowercase())
    }
}

// ---------------------------------------------------------------------------
// Character n-gram tokenizer
// ---------------------------------------------------------------------------

/// Character n-gram tokenizer: splits on non-alphanumeric (unicode-aware),
/// lowercases, then emits sliding character n-grams of length `n` per word.
/// A word shorter than `n` is emitted whole (so short words remain findable).
/// `n == 0` is treated as `1`. Enables substring / CJK / fuzzy matching where
/// word-boundary tokenization fails.
pub struct NgramTokenizer {
    n: usize,
}

impl NgramTokenizer {
    pub fn new(n: u8) -> Self {
        Self {
            n: (n as usize).max(1),
        }
    }
}

impl Tokenizer for NgramTokenizer {
    fn tokenize<'a>(&self, text: &'a str) -> Vec<Cow<'a, str>> {
        let mut tokens = Vec::new();
        // Iterate words the same way UnicodeTokenizer does (alphanumeric runs).
        let mut start = None;
        for (i, ch) in text.char_indices() {
            if ch.is_alphanumeric() {
                if start.is_none() {
                    start = Some(i);
                }
            } else if let Some(s) = start.take() {
                emit_ngrams(&text[s..i], self.n, &mut tokens);
            }
        }
        if let Some(s) = start {
            emit_ngrams(&text[s..], self.n, &mut tokens);
        }
        tokens
    }
}

/// Lowercase a word, then emit character n-grams (or the whole word if
/// shorter than `n`).
fn emit_ngrams<'a>(word: &str, n: usize, out: &mut Vec<Cow<'a, str>>) {
    let lowered = word.to_lowercase();
    let chars: Vec<char> = lowered.chars().collect();
    if chars.len() <= n {
        out.push(Cow::Owned(lowered));
    } else {
        for i in 0..=chars.len() - n {
            let gram: String = chars[i..i + n].iter().collect();
            out.push(Cow::Owned(gram));
        }
    }
}

// ---------------------------------------------------------------------------
// Full pipeline tokenizer (stopwords + stemming)
// ---------------------------------------------------------------------------

/// Returns the curated stopword set for a language, if one exists.
///
/// Currently only English and Russian have curated stopword lists.
/// For all other Snowball languages the function returns `None`;
/// stemming still works — stopword filtering is simply skipped.
/// Non-EN/RU stopword lists are a future addition.
fn stopwords_for(lang: StemLanguage) -> Option<&'static std::collections::HashSet<&'static str>> {
    match lang {
        StemLanguage::English => Some(english_stopwords()),
        StemLanguage::Russian => Some(russian_stopwords()),
        StemLanguage::Arabic
        | StemLanguage::Danish
        | StemLanguage::Dutch
        | StemLanguage::Finnish
        | StemLanguage::French
        | StemLanguage::German
        | StemLanguage::Greek
        | StemLanguage::Hungarian
        | StemLanguage::Italian
        | StemLanguage::Norwegian
        | StemLanguage::Portuguese
        | StemLanguage::Romanian
        | StemLanguage::Spanish
        | StemLanguage::Swedish
        | StemLanguage::Tamil
        | StemLanguage::Turkish => None,
    }
}

/// Map [`StemLanguage`] → [`rust_stemmers::Algorithm`].
/// Exhaustive — no wildcard arm, so adding a new variant to
/// `StemLanguage` is a compile error until mapped here.
fn stem_algorithm(lang: StemLanguage) -> rust_stemmers::Algorithm {
    match lang {
        StemLanguage::English => rust_stemmers::Algorithm::English,
        StemLanguage::Russian => rust_stemmers::Algorithm::Russian,
        StemLanguage::Arabic => rust_stemmers::Algorithm::Arabic,
        StemLanguage::Danish => rust_stemmers::Algorithm::Danish,
        StemLanguage::Dutch => rust_stemmers::Algorithm::Dutch,
        StemLanguage::Finnish => rust_stemmers::Algorithm::Finnish,
        StemLanguage::French => rust_stemmers::Algorithm::French,
        StemLanguage::German => rust_stemmers::Algorithm::German,
        StemLanguage::Greek => rust_stemmers::Algorithm::Greek,
        StemLanguage::Hungarian => rust_stemmers::Algorithm::Hungarian,
        StemLanguage::Italian => rust_stemmers::Algorithm::Italian,
        StemLanguage::Norwegian => rust_stemmers::Algorithm::Norwegian,
        StemLanguage::Portuguese => rust_stemmers::Algorithm::Portuguese,
        StemLanguage::Romanian => rust_stemmers::Algorithm::Romanian,
        StemLanguage::Spanish => rust_stemmers::Algorithm::Spanish,
        StemLanguage::Swedish => rust_stemmers::Algorithm::Swedish,
        StemLanguage::Tamil => rust_stemmers::Algorithm::Tamil,
        StemLanguage::Turkish => rust_stemmers::Algorithm::Turkish,
    }
}

/// Full-pipeline FTS tokenizer: whitespace split → lowercase →
/// optional language-specific stopwords → optional snowball stemming.
///
/// Read-only after construction; safe to share across threads without
/// any mutex.
pub struct FullTokenizer {
    stopwords: Option<&'static std::collections::HashSet<&'static str>>,
    stemmer: Option<rust_stemmers::Stemmer>,
}

impl FullTokenizer {
    pub fn new(language: StemLanguage, stopwords: bool, stem: bool) -> Self {
        let stop_set = if stopwords {
            stopwords_for(language)
        } else {
            None
        };
        let stemmer = if stem {
            Some(rust_stemmers::Stemmer::create(stem_algorithm(language)))
        } else {
            None
        };
        Self {
            stopwords: stop_set,
            stemmer,
        }
    }
}

impl Tokenizer for FullTokenizer {
    fn tokenize<'a>(&self, text: &'a str) -> Vec<Cow<'a, str>> {
        let mut out = Vec::new();
        for word in text.split_whitespace() {
            let lowered: Cow<'a, str> = if word
                .bytes()
                .all(|b| b.is_ascii_lowercase() || !b.is_ascii_alphabetic())
            {
                Cow::Borrowed(word)
            } else {
                Cow::Owned(word.to_lowercase())
            };
            if let Some(sw) = self.stopwords {
                if sw.contains(lowerd_str(&lowered)) {
                    continue;
                }
            }
            if let Some(ref stemmer) = self.stemmer {
                let stemmed = stemmer.stem(lowerd_str(&lowered)).into_owned();
                out.push(Cow::Owned(stemmed));
            } else {
                out.push(lowered);
            }
        }
        out
    }
}

fn lowerd_str<'a>(cow: &'a Cow<'a, str>) -> &'a str {
    cow.as_ref()
}

fn english_stopwords() -> &'static std::collections::HashSet<&'static str> {
    use std::sync::OnceLock;
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into",
            "is", "it", "no", "not", "of", "on", "or", "such", "that", "the", "their", "then",
            "there", "these", "they", "this", "to", "was", "will", "with",
        ]
        .iter()
        .copied()
        .collect()
    })
}

fn russian_stopwords() -> &'static std::collections::HashSet<&'static str> {
    use std::sync::OnceLock;
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            "и",
            "в",
            "во",
            "не",
            "что",
            "он",
            "на",
            "я",
            "с",
            "со",
            "как",
            "а",
            "то",
            "все",
            "она",
            "так",
            "его",
            "но",
            "да",
            "ты",
            "к",
            "у",
            "же",
            "вы",
            "за",
            "бы",
            "по",
            "только",
            "её",
            "мне",
            "было",
            "вот",
            "от",
            "меня",
            "ещё",
            "нет",
            "о",
            "из",
            "ему",
            "теперь",
            "когда",
            "даже",
            "ну",
            "ли",
            "если",
            "уже",
            "или",
            "ни",
            "быть",
            "был",
            "него",
            "до",
            "вас",
            "нибудь",
            "опять",
            "уж",
            "вам",
            "ведь",
            "там",
            "потом",
            "себя",
            "ничего",
            "ей",
            "может",
            "они",
            "тут",
            "где",
            "есть",
            "надо",
            "ней",
            "для",
            "мы",
            "тебя",
            "их",
            "чем",
            "была",
            "сам",
            "чтоб",
            "без",
            "будто",
            "чего",
            "раз",
            "тоже",
            "себе",
            "под",
            "будет",
            "ж",
            "тогда",
            "кто",
            "этот",
            "того",
            "потому",
            "этого",
            "какой",
            "совсем",
            "ним",
            "здесь",
            "этом",
            "один",
            "почти",
            "мой",
            "тем",
            "чтобы",
            "нее",
            "сейчас",
            "были",
            "туда",
            "откуда",
            "этой",
            "перед",
            "иногда",
            "ведь",
            "тоже",
        ]
        .iter()
        .copied()
        .collect()
    })
}

/// Build a `Box<dyn Tokenizer>` from a [`TokenizerKind`].
pub fn build_tokenizer(kind: &crate::index2::kind::TokenizerKind) -> Box<dyn Tokenizer> {
    match kind {
        crate::index2::kind::TokenizerKind::Whitespace => Box::new(WhitespaceTokenizer),
        crate::index2::kind::TokenizerKind::Unicode => Box::new(UnicodeTokenizer),
        crate::index2::kind::TokenizerKind::Ngram { n } => Box::new(NgramTokenizer::new(*n)),
        crate::index2::kind::TokenizerKind::Full {
            language,
            stopwords,
            stem,
        } => Box::new(FullTokenizer::new(*language, *stopwords, *stem)),
    }
}

/// Hash a token to u64 for posting keys.
pub fn token_hash(token: &str) -> u64 {
    use fxhash::FxHasher;
    use std::hash::Hasher;
    let mut h = FxHasher::default();
    h.write(token.as_bytes());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_basic() {
        let t = WhitespaceTokenizer;
        let tokens = t.tokenize("Hello World  foo");
        assert_eq!(tokens, vec!["hello", "world", "foo"]);
    }

    #[test]
    fn whitespace_already_lowercase() {
        let t = WhitespaceTokenizer;
        let tokens = t.tokenize("already lower 123");
        assert!(tokens.iter().all(|t| matches!(t, Cow::Borrowed(_))));
    }

    #[test]
    fn whitespace_empty() {
        let t = WhitespaceTokenizer;
        assert!(t.tokenize("").is_empty());
        assert!(t.tokenize("   ").is_empty());
    }

    #[test]
    fn unicode_splits_on_punctuation() {
        let t = UnicodeTokenizer;
        let tokens = t.tokenize("alice@example.com — hello!");
        assert_eq!(tokens, vec!["alice", "example", "com", "hello"]);
    }

    #[test]
    fn unicode_cyrillic() {
        let t = UnicodeTokenizer;
        let tokens = t.tokenize("Привет Мир");
        assert_eq!(tokens, vec!["привет", "мир"]);
    }

    #[test]
    fn unicode_mixed() {
        let t = UnicodeTokenizer;
        let tokens = t.tokenize("foo123 BAR baz");
        assert_eq!(tokens, vec!["foo123", "bar", "baz"]);
    }

    #[test]
    fn token_hash_deterministic() {
        assert_eq!(token_hash("hello"), token_hash("hello"));
        assert_ne!(token_hash("hello"), token_hash("world"));
    }

    // ----- Full tokenizer: stemming -----

    #[test]
    fn english_stem_normalises_runs() {
        let t = FullTokenizer::new(StemLanguage::English, false, true);
        let running = t.tokenize("running");
        let runs = t.tokenize("runs");
        assert_eq!(
            running, runs,
            "running and runs should stem to the same token"
        );
    }

    #[test]
    fn russian_stem_basic() {
        let t = FullTokenizer::new(StemLanguage::Russian, false, true);
        let tokens = t.tokenize("бежал");
        assert!(!tokens.is_empty(), "should produce at least one token");
        let stemmed = &tokens[0];
        assert!(!stemmed.is_empty());
    }

    // ----- Full tokenizer: stopwords -----

    #[test]
    fn english_stopwords_filtered() {
        let t = FullTokenizer::new(StemLanguage::English, true, false);
        let tokens = t.tokenize("the cat");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert_eq!(strs, vec!["cat"], "the should be filtered as stopword");
    }

    #[test]
    fn russian_stopwords_filtered() {
        let t = FullTokenizer::new(StemLanguage::Russian, true, false);
        let tokens = t.tokenize("и кот");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert_eq!(strs, vec!["кот"], "и should be filtered as stopword");
    }

    // ----- build_tokenizer helper -----

    #[test]
    fn build_tokenizer_whitespace() {
        let t = build_tokenizer(&crate::index2::kind::TokenizerKind::Whitespace);
        let tokens = t.tokenize("Hello World");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn build_tokenizer_full_en() {
        use crate::index2::kind::TokenizerKind;
        let t = build_tokenizer(&TokenizerKind::Full {
            language: StemLanguage::English,
            stopwords: true,
            stem: true,
        });
        let tokens = t.tokenize("the cats are running");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert!(!strs.contains(&"the"), "stopword 'the' should be filtered");
        assert!(!strs.contains(&"are"), "stopword 'are' should be filtered");
    }

    // ----- Stemming: new languages -----

    #[test]
    fn french_stem_inflections() {
        let t = FullTokenizer::new(StemLanguage::French, false, true);
        let a = t.tokenize("chats");
        let b = t.tokenize("chat");
        assert_eq!(a, b, "French: chats and chat should stem identically");
    }

    #[test]
    fn german_stem_produces_output() {
        let t = FullTokenizer::new(StemLanguage::German, false, true);
        let tokens = t.tokenize("laufen");
        assert!(!tokens.is_empty());
        assert!(!tokens[0].is_empty());
    }

    #[test]
    fn spanish_stem_inflections() {
        let t = FullTokenizer::new(StemLanguage::Spanish, false, true);
        let a = t.tokenize("corriendo");
        let b = t.tokenize("corre");
        // Both should produce non-empty stems; Snowball may or may not
        // unify them but they must be non-empty.
        assert!(!a.is_empty());
        assert!(!b.is_empty());
    }

    #[test]
    fn italian_stem_produces_output() {
        let t = FullTokenizer::new(StemLanguage::Italian, false, true);
        let tokens = t.tokenize("correndo");
        assert!(!tokens.is_empty());
        assert!(!tokens[0].is_empty());
    }

    #[test]
    fn stopwords_graceful_for_unsupported_language() {
        // French has no curated stopword set — requesting stopwords
        // should be a no-op (no error, no filtering).
        let t = FullTokenizer::new(StemLanguage::French, true, false);
        let tokens = t.tokenize("le chat");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        // "le" is NOT filtered because there's no French stopword list.
        assert_eq!(strs, vec!["le", "chat"]);
    }

    // ----- NgramTokenizer -----

    #[test]
    fn ngram_hello_n3() {
        let t = NgramTokenizer::new(3);
        let tokens = t.tokenize("hello");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert_eq!(strs, vec!["hel", "ell", "llo"]);
    }

    #[test]
    fn ngram_short_word() {
        let t = NgramTokenizer::new(3);
        let tokens = t.tokenize("hi");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert_eq!(strs, vec!["hi"], "word shorter than n emitted whole");
    }

    #[test]
    fn ngram_n2_abc() {
        let t = NgramTokenizer::new(2);
        let tokens = t.tokenize("abc");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert_eq!(strs, vec!["ab", "bc"]);
    }

    #[test]
    fn ngram_splits_on_punctuation() {
        let t = NgramTokenizer::new(2);
        let tokens = t.tokenize("ab-cd");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert_eq!(strs, vec!["ab", "cd"]);
    }

    #[test]
    fn ngram_zero_treated_as_one() {
        let t = NgramTokenizer::new(0);
        let tokens = t.tokenize("hi");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert_eq!(strs, vec!["h", "i"]);
    }

    #[test]
    fn ngram_lowercases() {
        let t = NgramTokenizer::new(3);
        let tokens = t.tokenize("HELLO");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert_eq!(strs, vec!["hel", "ell", "llo"]);
    }

    // ----- build_tokenizer: ngram + full French -----

    #[test]
    fn build_tokenizer_ngram() {
        use crate::index2::kind::TokenizerKind;
        let t = build_tokenizer(&TokenizerKind::Ngram { n: 3 });
        let tokens = t.tokenize("hello");
        let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
        assert_eq!(strs, vec!["hel", "ell", "llo"]);
    }

    #[test]
    fn build_tokenizer_full_french() {
        use crate::index2::kind::TokenizerKind;
        let t = build_tokenizer(&TokenizerKind::Full {
            language: StemLanguage::French,
            stopwords: true,
            stem: true,
        });
        let tokens = t.tokenize("chats");
        assert!(!tokens.is_empty(), "French stemmer should produce output");
    }
}
