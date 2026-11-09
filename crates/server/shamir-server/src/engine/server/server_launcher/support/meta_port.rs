use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::time::UnixNanos;
use shamir_sync::meta_port::{BootstrapMeta, BootstrapTokenSnapshot, MetaError};

use crate::server_meta::ServerMetaStore;
#[cfg(test)]
use crate::server_meta::CLOCK_OVERRIDE_NS;
use crate::user_directory::FjallUserDirectory;

impl BootstrapMeta for ServerMetaStore {
    fn now_unix_nanos(&self) -> Result<u64, shamir_connect::common::error::EnvironmentError> {
        #[cfg(test)]
        {
            let pinned = CLOCK_OVERRIDE_NS.load(std::sync::atomic::Ordering::Relaxed);
            if pinned != 0 {
                return Ok(pinned);
            }
        }
        UnixNanos::try_now().map(|now| now.as_u64())
    }

    fn read_bootstrap_token_state(&self, now_ns: u64) -> Result<BootstrapTokenSnapshot, MetaError> {
        ServerMetaStore::read_bootstrap_token_state(self, now_ns)
    }

    fn bootstrap_token_active(&self) -> bool {
        ServerMetaStore::bootstrap_token_active(self)
    }

    fn bootstrap_username(&self) -> Option<String> {
        ServerMetaStore::bootstrap_username(self)
    }

    fn bootstrap_token_path(&self) -> Option<PathBuf> {
        ServerMetaStore::bootstrap_token_path(self)
    }

    fn set_bootstrap_token(
        &self,
        username: &str,
        token_hash: [u8; 32],
        expires_at_ns: u64,
        token_path: PathBuf,
    ) -> Result<(), MetaError> {
        ServerMetaStore::set_bootstrap_token(self, username, token_hash, expires_at_ns, token_path)
    }

    fn consume_bootstrap_token(&self) -> Result<(), MetaError> {
        ServerMetaStore::consume_bootstrap_token(self)
    }

    fn rotate_bootstrap_credential_to_random<'a>(
        &self,
        user_dir: &'a FjallUserDirectory,
        username: &'a str,
        kdf: KdfParams,
        now_ns: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(crate::bootstrap::rotate_bootstrap_credential_to_random(
            user_dir, username, kdf, now_ns,
        ))
    }
}
