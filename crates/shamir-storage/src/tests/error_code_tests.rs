//! RU8 (RV-TYPES-2EB39DFD): the typed interner quota error must map to the
//! `interner_quota_exceeded` wire code - both directly and through codec
//! call chains (`CodecError::Interner`).

use crate::core::error::DbError;
use crate::storage_cached::CachedStore;
#[cfg(feature = "fjall")]
use crate::storage_fjall::FjallRepo;
use crate::storage_in_memory::InMemoryStore;
use crate::storage_membuffer::{MemBufferConfig, MemBufferStore};
#[cfg(feature = "fjall")]
use crate::types::Repo;
use crate::types::{RecordKey, Store};
use shamir_types::codecs::CodecError;
use shamir_types::core::interner::{InternerError, InternerQuotaKind};
use std::sync::Arc;

#[test]
fn rv_types_2eb39dfd_quota_error_maps_to_wire_code() {
    let err = DbError::from(InternerError::QuotaExceeded {
        label: "repo-17".to_string(),
        kind: InternerQuotaKind::Names,
        limit: 7,
    });
    assert!(matches!(
        &err,
        DbError::InternerQuotaExceeded {
            repo,
            kind: InternerQuotaKind::Names,
            limit: 7
        } if repo == "repo-17"
    ));
    assert_eq!(err.code(), Some("interner_quota_exceeded"));
    let message = err.to_string();
    assert!(message.contains("repo-17"));
    assert!(message.contains("Names"));
    assert!(message.contains("7"));
    assert!(message.contains("SHAMIR_INTERNER_MAX_NAMES"));
    assert!(message.contains("SHAMIR_INTERNER_MAX_NAME_BYTES"));

    let via_codec = DbError::from(CodecError::Interner(InternerError::QuotaExceeded {
        label: "repo-17".to_string(),
        kind: InternerQuotaKind::Bytes,
        limit: 9,
    }));
    assert_eq!(via_codec.code(), Some("interner_quota_exceeded"));
}

#[tokio::test]
async fn missing_key_diagnostics_are_bounded_across_backends() {
    let key = RecordKey::from_slice(&vec![255; 8192]);
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let cached = CachedStore::new_sync(inner.clone()).await.unwrap();
    let buffered = MemBufferStore::new(inner.clone(), MemBufferConfig::default());
    buffered.disable_background_flusher_for_test();
    let check = |error: DbError| {
        let DbError::NotFound(message) = error else {
            panic!("expected NotFound")
        };
        assert!(message.len() <= 80, "diagnostic length: {}", message.len());
        assert!(message.contains("8192"));
        assert!(!message.contains("255, 255"));
    };
    check(inner.get(key.clone()).await.unwrap_err());
    check(cached.get(key.clone()).await.unwrap_err());
    check(buffered.get(key.clone()).await.unwrap_err());
    check(buffered.get(key.clone()).await.unwrap_err());
    buffered.remove(key.clone()).await.unwrap();
    check(buffered.get(key.clone()).await.unwrap_err());

    #[cfg(feature = "fjall")]
    {
        let dir = tempfile::tempdir().unwrap();
        let repo = FjallRepo::new(dir.path()).unwrap();
        let store = repo.store_get("diagnostic").await.unwrap();
        check(store.get(key).await.unwrap_err());
    }
}
