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
    Ngram { n: u8 },
}

/// Placeholder for the Phase 1 `IndexExpr` AST — kept as opaque
/// bytes so Phase 0 doesn't have to ship the full expression
/// language. Phase 1 will replace this with a structured variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionalConfig {
    pub expr_serialized: Vec<u8>,
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
}
