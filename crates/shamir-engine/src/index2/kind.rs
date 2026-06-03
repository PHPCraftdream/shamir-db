//! Discriminated index variants.
//!
//! Stored in `IndexDescriptor.kind`; serialized through `MetaEnvelope`
//! to `__meta__/indexes`. Big variants are `Box`-ed so the enum stays
//! compact on the hot path.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexKind {
    Btree {
        unique: bool,
    },
    Fts {
        tokenizer: TokenizerKind,
        language: Option<String>,
    },
    Functional(Box<FunctionalConfig>),
    Vector(Box<VectorConfig>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TokenizerKind {
    Whitespace,
    Unicode,
    Ngram {
        n: u8,
    },
    /// Full pipeline: whitespace split → lowercase → optional stopwords
    /// → optional snowball stemming.  `language` selects both the
    /// stopword list and the stemmer algorithm.
    Full {
        language: StemLanguage,
        #[serde(default = "default_true")]
        stopwords: bool,
        #[serde(default = "default_true")]
        stem: bool,
    },
}

/// Language selector for [`TokenizerKind::Full`].
///
/// Maps 1:1 to `rust_stemmers::Algorithm` and to a built-in stopword
/// list (where available).  Serialises as a lowercase string
/// (`"english"` / `"russian"` / ...).
///
/// # Bincode ordinal stability: append only
///
/// This enum is persisted via bincode which encodes variants by their
/// ordinal position.  **Never** reorder or insert variants before
/// existing ones — only append new languages at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StemLanguage {
    // ordinal 0 — DO NOT MOVE
    English,
    // ordinal 1 — DO NOT MOVE
    Russian,
    // --- new languages appended below (ordinal 2+) ---
    Arabic,
    Danish,
    Dutch,
    Finnish,
    French,
    German,
    Greek,
    Hungarian,
    Italian,
    Norwegian,
    Portuguese,
    Romanian,
    Spanish,
    Swedish,
    Tamil,
    Turkish,
}

impl StemLanguage {
    /// Parse a DSL string into a [`StemLanguage`].
    ///
    /// Accepts both the full lowercase name and the ISO 639-1
    /// two-letter code:
    ///
    /// ```text
    /// "english" | "en"  → English
    /// "russian" | "ru"  → Russian
    /// "french"  | "fr"  → French
    ///   ...etc...
    /// ```
    ///
    /// Returns `None` for unrecognised strings.
    pub fn from_dsl(s: &str) -> Option<Self> {
        match s {
            "english" | "en" => Some(Self::English),
            "russian" | "ru" => Some(Self::Russian),
            "arabic" | "ar" => Some(Self::Arabic),
            "danish" | "da" => Some(Self::Danish),
            "dutch" | "nl" => Some(Self::Dutch),
            "finnish" | "fi" => Some(Self::Finnish),
            "french" | "fr" => Some(Self::French),
            "german" | "de" => Some(Self::German),
            "greek" | "el" => Some(Self::Greek),
            "hungarian" | "hu" => Some(Self::Hungarian),
            "italian" | "it" => Some(Self::Italian),
            "norwegian" | "no" => Some(Self::Norwegian),
            "portuguese" | "pt" => Some(Self::Portuguese),
            "romanian" | "ro" => Some(Self::Romanian),
            "spanish" | "es" => Some(Self::Spanish),
            "swedish" | "sv" => Some(Self::Swedish),
            "tamil" | "ta" => Some(Self::Tamil),
            "turkish" | "tr" => Some(Self::Turkish),
            _ => None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionalConfig {
    pub expr: crate::index2::expr::IndexExpr,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum VectorMetric {
    L2,
    Cosine,
    Dot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorConfig {
    pub dim: u32,
    pub metric: VectorMetric,
    pub backend: VectorBackendRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorBackendRef {
    InProcessHnsw {
        ef_construct: u32,
        m: u32,
    },
    External {
        driver: String,
        url: String,
        api_key_secret: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_btree() {
        let k = IndexKind::Btree { unique: true };
        let bytes = bincode::serialize(&k).unwrap();
        let got: IndexKind = bincode::deserialize(&bytes).unwrap();
        assert!(matches!(got, IndexKind::Btree { unique: true }));
    }

    #[test]
    fn serde_round_trip_vector() {
        let k = IndexKind::Vector(Box::new(VectorConfig {
            dim: 384,
            metric: VectorMetric::Cosine,
            backend: VectorBackendRef::InProcessHnsw {
                ef_construct: 200,
                m: 16,
            },
        }));
        let bytes = bincode::serialize(&k).unwrap();
        let got: IndexKind = bincode::deserialize(&bytes).unwrap();
        match got {
            IndexKind::Vector(c) => assert_eq!(c.dim, 384),
            _ => panic!("wrong variant"),
        }
    }

    // ----- StemLanguage ordinal stability -----

    #[test]
    fn stem_language_english_ordinal_zero() {
        let bytes = bincode::serialize(&StemLanguage::English).unwrap();
        let ordinal: u32 = bincode::deserialize(&bytes).unwrap();
        assert_eq!(ordinal, 0, "English must remain ordinal 0");
    }

    #[test]
    fn stem_language_russian_ordinal_one() {
        let bytes = bincode::serialize(&StemLanguage::Russian).unwrap();
        let ordinal: u32 = bincode::deserialize(&bytes).unwrap();
        assert_eq!(ordinal, 1, "Russian must remain ordinal 1");
    }

    // ----- StemLanguage::from_dsl -----

    #[test]
    fn from_dsl_full_names() {
        assert_eq!(
            StemLanguage::from_dsl("english"),
            Some(StemLanguage::English)
        );
        assert_eq!(
            StemLanguage::from_dsl("russian"),
            Some(StemLanguage::Russian)
        );
        assert_eq!(StemLanguage::from_dsl("french"), Some(StemLanguage::French));
        assert_eq!(StemLanguage::from_dsl("german"), Some(StemLanguage::German));
        assert_eq!(
            StemLanguage::from_dsl("spanish"),
            Some(StemLanguage::Spanish)
        );
        assert_eq!(StemLanguage::from_dsl("arabic"), Some(StemLanguage::Arabic));
        assert_eq!(StemLanguage::from_dsl("tamil"), Some(StemLanguage::Tamil));
    }

    #[test]
    fn from_dsl_two_letter_codes() {
        assert_eq!(StemLanguage::from_dsl("en"), Some(StemLanguage::English));
        assert_eq!(StemLanguage::from_dsl("ru"), Some(StemLanguage::Russian));
        assert_eq!(StemLanguage::from_dsl("fr"), Some(StemLanguage::French));
        assert_eq!(StemLanguage::from_dsl("de"), Some(StemLanguage::German));
        assert_eq!(StemLanguage::from_dsl("es"), Some(StemLanguage::Spanish));
        assert_eq!(StemLanguage::from_dsl("nl"), Some(StemLanguage::Dutch));
        assert_eq!(StemLanguage::from_dsl("sv"), Some(StemLanguage::Swedish));
        assert_eq!(StemLanguage::from_dsl("no"), Some(StemLanguage::Norwegian));
        assert_eq!(StemLanguage::from_dsl("da"), Some(StemLanguage::Danish));
        assert_eq!(StemLanguage::from_dsl("fi"), Some(StemLanguage::Finnish));
        assert_eq!(StemLanguage::from_dsl("hu"), Some(StemLanguage::Hungarian));
        assert_eq!(StemLanguage::from_dsl("ro"), Some(StemLanguage::Romanian));
        assert_eq!(StemLanguage::from_dsl("tr"), Some(StemLanguage::Turkish));
        assert_eq!(StemLanguage::from_dsl("el"), Some(StemLanguage::Greek));
        assert_eq!(StemLanguage::from_dsl("ar"), Some(StemLanguage::Arabic));
        assert_eq!(StemLanguage::from_dsl("ta"), Some(StemLanguage::Tamil));
        assert_eq!(StemLanguage::from_dsl("pt"), Some(StemLanguage::Portuguese));
        assert_eq!(StemLanguage::from_dsl("it"), Some(StemLanguage::Italian));
    }

    #[test]
    fn from_dsl_unknown_returns_none() {
        assert_eq!(StemLanguage::from_dsl("klingon"), None);
        assert_eq!(StemLanguage::from_dsl(""), None);
        assert_eq!(StemLanguage::from_dsl("EN"), None); // case-sensitive
    }
}
