//! Identity-seam adapter (task #559): implements shamir-db's
//! [`PrincipalResolver`] + [`UserAdminPort`] over the real
//! [`FjallUserDirectory`]. Injected into [`ShamirDb`] at boot so the
//! engine's user-admin handlers and principal introspection reach the
//! durable directory instead of shamir-db's own retired Store B.
//!
//! [`ShamirDb`]: shamir_db::ShamirDb

use std::sync::Arc;

use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::admin::UserDirectory as _;
use shamir_db::{PortError, PrincipalInfo, PrincipalResolver, UserAdminPort};

use crate::db_handler::derive_scram_record;
use crate::user_directory::FjallUserDirectory;

/// Thin adapter wrapping a shared [`FjallUserDirectory`] handle. Carries the
/// KDF params used for new-user Argon2id derivation (mirrors `AdminGlue`'s
/// `kdf` field — the port owns user creation end-to-end, so it needs its
/// own copy rather than reaching back into `AdminGlue`).
pub struct DirectoryPorts {
    dir: Arc<FjallUserDirectory>,
    kdf: KdfParams,
}

impl DirectoryPorts {
    pub fn new(dir: Arc<FjallUserDirectory>, kdf: KdfParams) -> Self {
        Self { dir, kdf }
    }

    /// Convenience: build the `Arc<dyn ...>` pair the boot path injects into
    /// `ShamirDb`. Both traits are implemented on the SAME object (one
    /// directory handle backs both), so a single `Arc` is shared.
    pub fn into_trait_objects(self) -> (Arc<dyn UserAdminPort>, Arc<dyn PrincipalResolver>) {
        let arc = Arc::new(self);
        (
            arc.clone() as Arc<dyn UserAdminPort>,
            arc as Arc<dyn PrincipalResolver>,
        )
    }
}

impl PrincipalResolver for DirectoryPorts {
    fn resolve(&self, principal64_key: u64) -> Option<PrincipalInfo> {
        let st = self.dir.resolve_by_principal64(principal64_key)?;
        Some(PrincipalInfo {
            principal64: principal64_key,
            name: st.username,
            user_id: st.user_id,
            database: st.database,
            superuser: st.superuser,
        })
    }

    fn list(&self) -> Vec<PrincipalInfo> {
        let entries = match self.dir.list_all() {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        entries
            .into_iter()
            .map(|(principal64, st)| PrincipalInfo {
                principal64,
                name: st.username,
                user_id: st.user_id,
                database: st.database,
                superuser: st.superuser,
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl UserAdminPort for DirectoryPorts {
    async fn create_user(
        &self,
        name: &str,
        password: &str,
        roles: Vec<String>,
        database: Option<String>,
    ) -> Result<[u8; 16], PortError> {
        // Reject the reserved "superuser" role BEFORE persisting anything —
        // `update_roles` enforces the same check (task #557), but checking
        // it here too avoids leaving a durably-persisted, roleless orphan
        // account behind a reported failure (the insert below cannot be
        // rolled back once committed).
        if roles.iter().any(|r| r == "superuser") {
            return Err(PortError::from(
                "\"superuser\" is a reserved role name — use SetSuperuser to grant/revoke superuser status"
                    .to_string(),
            ));
        }
        // Argon2id derivation reuses the shared helper factored out of
        // `create_scram_user` (task #559 brief §3: don't duplicate a third
        // time). shamir-db never touches SCRAM crypto.
        let record = derive_scram_record(password.to_string(), self.kdf)
            .await
            .map_err(PortError::from)?;
        let user_id = self
            .dir
            .insert_with_scope(name.to_string(), record, database)
            .map_err(|e| PortError::from(e.to_string()))?;
        if !roles.is_empty() {
            // now_ns=0: a brand-new user has no live sessions to invalidate.
            // The reserved-role check above means this can only fail on an
            // internal directory error, not on caller input.
            self.dir
                .update_roles(name, roles, 0)
                .map_err(|e| PortError::from(e.to_string()))?;
        }
        Ok(user_id)
    }

    async fn drop_user(&self, name: &str) -> Result<bool, PortError> {
        self.dir
            .remove(name)
            .map_err(|e| PortError::from(e.to_string()))
    }

    async fn grant_role(&self, user: &str, role: &str) -> Result<(), PortError> {
        let now_ns = UnixNanos::now().as_u64();
        self.dir
            .grant_role(user, role, now_ns)
            .map(|_| ())
            .map_err(|e| PortError::from(e.to_string()))
    }

    async fn revoke_role(&self, user: &str, role: &str) -> Result<(), PortError> {
        let now_ns = UnixNanos::now().as_u64();
        self.dir
            .revoke_role(user, role, now_ns)
            .map(|_| ())
            .map_err(|e| PortError::from(e.to_string()))
    }

    async fn set_superuser(&self, user: &str, on: bool) -> Result<(), PortError> {
        let now_ns = UnixNanos::now().as_u64();
        self.dir
            .set_superuser(user, on, now_ns)
            .map(|_| ())
            .map_err(|e| PortError::from(e.to_string()))
    }
}
