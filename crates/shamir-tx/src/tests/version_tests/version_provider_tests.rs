use crate::version_provider::VersionProvider;
use bytes::Bytes;
use std::sync::Arc;

struct StubProvider {
    version: u64,
}

impl VersionProvider for StubProvider {
    fn version_of(&self, _table_id: u64, _key: &Bytes) -> Option<u64> {
        Some(self.version)
    }
}

#[test]
fn stub_provider_returns_configured_version() {
    let p: Arc<dyn VersionProvider> = Arc::new(StubProvider { version: 99 });
    assert_eq!(p.version_of(0, &Bytes::from_static(b"k")), Some(99));
}
