use crate::kind::{IndexKind, StemLanguage, VectorBackendRef, VectorConfig, VectorMetric};

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
