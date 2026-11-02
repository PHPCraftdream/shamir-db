use super::audit_context::TokenAuditContext;
use super::gc::GcService;
use super::new_token::NewToken;
use super::options::{TokenGenerator, TokenServiceOptions};
use super::session_canceller::{NoopCanceller, SessionCanceller};
use crate::security::tokens::{
    CreatedToken, TokenCounters, TokenCreateError, TokenStore, TokenStoreError, TokenView,
};
use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::token::{
    compute_mac, encode_token, generate_token_parts, verify_token, Endpoint, OwnerFloors, TokenId,
    TokenMirror, TokenRecord, VerifyCtx, VerifyFailure,
};
use shamir_connect::server::TokenSessionStore;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

/// Durable API-token service: issue, verify, revoke, list and retention GC.
pub struct TokenService {
    store: TokenStore,
    mirror: Arc<TokenMirror>,
    token_key: Zeroizing<[u8; 32]>,
    owners: Arc<dyn OwnerFloors + Send + Sync>,
    sessions: Arc<TokenSessionStore>,
    canceller: Arc<dyn SessionCanceller>,
    audit: Arc<AuditChainWriter>,
    counters: Arc<TokenCounters>,
    default_ttl_days: u64,
    max_ttl_days: u64,
    max_per_user: usize,
    read_only_node: bool,
    mutation_gates: Arc<[Arc<AsyncMutex<()>>; 64]>,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    gc_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    gc_shutdown: CancellationToken,
    mirror_insert_conflict: Arc<std::sync::atomic::AtomicBool>,
    test_generator: Arc<TokenGenerator>,
}
impl std::fmt::Debug for TokenService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenService")
            .field("store", &self.store)
            .field("token_key", &"<REDACTED>")
            .finish()
    }
}
impl TokenService {
    /// Open storage, validate its full scan, load live records into the mirror, and run GC.
    pub async fn open(options: TokenServiceOptions) -> Result<Self, TokenStoreError> {
        let TokenServiceOptions {
            data_dir,
            token_key,
            owners,
            sessions,
            audit,
            config,
            read_only_node,
            clock,
        } = options;
        let store = TokenStore::open(data_dir).await?;
        let now = clock();
        let records = store.scan().await?;
        let service = Self {
            store,
            mirror: Arc::new(TokenMirror::new()),
            token_key,
            owners,
            sessions,
            canceller: Arc::new(NoopCanceller),
            audit,
            counters: Arc::new(TokenCounters::default()),
            default_ttl_days: config.default_ttl_days,
            max_ttl_days: config.max_ttl_days,
            max_per_user: config.max_per_user,
            read_only_node,
            mutation_gates: Arc::new(std::array::from_fn(|_| Arc::new(AsyncMutex::new(())))),
            clock,
            gc_task: tokio::sync::Mutex::new(None),
            gc_shutdown: CancellationToken::new(),
            mirror_insert_conflict: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            test_generator: Arc::new(generate_token_parts),
        };
        let mut records = records
            .into_iter()
            .filter(|r| r.revoked_at_ns.is_none() && now.is_some_and(|n| r.expires_at_ns > n))
            .collect::<Vec<_>>();
        service.mirror.load_all(records.drain(..));
        if !read_only_node {
            service.collect_garbage_now().await?;
        }
        Ok(service)
    }
    pub async fn open_with_clock(options: TokenServiceOptions) -> Result<Self, TokenStoreError> {
        Self::open(options).await
    }
    pub fn with_canceller(mut self, canceller: Arc<dyn SessionCanceller>) -> Self {
        self.canceller = canceller;
        self
    }
    #[cfg(test)]
    pub(crate) fn force_mirror_insert_conflict(&self) {
        self.mirror_insert_conflict
            .store(true, std::sync::atomic::Ordering::Release);
    }
    /// Replace OS-backed token generation in test fixtures.
    #[cfg(test)]
    pub(crate) fn inject_generator(&mut self, generator: Arc<TokenGenerator>) {
        self.test_generator = generator;
    }
    #[cfg(test)]
    pub(crate) fn store_for_tests(&self) -> &TokenStore {
        &self.store
    }
    /// Current service clock timestamp used by issuance and revocation decisions.
    pub fn now_ns(&self) -> Option<u64> {
        (self.clock)()
    }
    pub fn sessions(&self) -> &Arc<TokenSessionStore> {
        &self.sessions
    }
    #[cfg(test)]
    pub(crate) fn floor_for_tests(&self, owner: &[u8; 16]) -> Option<u64> {
        self.owners.floor_if_present(owner)
    }
    pub fn mirror(&self) -> &Arc<TokenMirror> {
        &self.mirror
    }
    pub fn counters(&self) -> &Arc<TokenCounters> {
        &self.counters
    }
    /// Current count of ACTIVE persisted tokens as of this timestamp.
    pub async fn active_count(&self, now_ns: u64) -> Result<u64, TokenStoreError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            Ok(store
                .scan_blocking()?
                .iter()
                .filter(|record| record.revoked_at_ns.is_none() && record.expires_at_ns > now_ns)
                .count() as u64)
        })
        .await
        .map_err(|_| TokenStoreError::Join)?
    }
    /// Cheap cardinality hint; includes expired and revoked mirror entries.
    pub fn mirror_entry_count_hint(&self) -> usize {
        self.mirror.len_hint()
    }

    /// Create, durably persist, and publish one token.
    pub async fn create(&self, input: NewToken) -> Result<CreatedToken, TokenCreateError> {
        self.create_with_context(input, TokenAuditContext::service_default())
            .await
    }
    /// Create, durably persist, and publish one token with explicit audit context.
    pub async fn create_with_context(
        &self,
        input: NewToken,
        audit_context: TokenAuditContext,
    ) -> Result<CreatedToken, TokenCreateError> {
        if self.read_only_node {
            return Err(TokenCreateError::ReadOnlyNode);
        }
        if input.label.is_empty()
            || input.label.len() > 64
            || input.owner_roles.len() > 64
            || input
                .owner_roles
                .iter()
                .any(|r| r.is_empty() || r.len() > 64)
            || input
                .databases
                .as_ref()
                .is_some_and(|d| d.len() > 64 || d.iter().any(|x| x.is_empty() || x.len() > 128))
        {
            return Err(TokenCreateError::InvalidInput);
        }
        let now = (self.clock)().ok_or(TokenCreateError::Clock)?;
        let ttl = input
            .ttl_secs
            .unwrap_or_else(|| self.default_ttl_days.saturating_mul(86400));
        let max = self
            .max_ttl_days
            .checked_mul(86400)
            .ok_or(TokenCreateError::TokenTtlInvalid)?;
        if ttl == 0 || ttl > max {
            return Err(TokenCreateError::TokenTtlInvalid);
        }
        let expires = now
            .checked_add(
                ttl.checked_mul(1_000_000_000)
                    .ok_or(TokenCreateError::TokenTtlInvalid)?,
            )
            .ok_or(TokenCreateError::TokenTtlInvalid)?;
        let gate = Arc::clone(&self.mutation_gates[usize::from(input.owner_user_id[0]) % 64])
            .lock_owned()
            .await;
        let store = self.store.clone();
        let mirror = Arc::clone(&self.mirror);
        let key = self.token_key.clone();
        let audit = Arc::clone(&self.audit);
        let counters = Arc::clone(&self.counters);
        let test_generator = Arc::clone(&self.test_generator);
        let mirror_insert_conflict = Arc::clone(&self.mirror_insert_conflict);
        let max_per_user = self.max_per_user;
        let result = tokio::task::spawn_blocking(move || {
            let _gate = gate;
            let existing = store
                .with_locked_records(|_, _| store.scan_locked())
                .map_err(TokenCreateError::Store)?
                .into_iter()
                .filter(|r| r.owner_user_id == input.owner_user_id)
                .collect::<Vec<_>>();
            let live = existing.iter().filter(|r| {
                r.owner_user_id == input.owner_user_id
                    && r.revoked_at_ns.is_none()
                    && r.expires_at_ns > now
            });
            if live.clone().any(|r| r.label == input.label) {
                return Err(TokenCreateError::LabelExists);
            }
            if live.count() >= max_per_user {
                return Err(TokenCreateError::LimitReached);
            }
            let mut generated = None;
            for _ in 0..8 {
                let (id, secret) = test_generator().map_err(|_| TokenCreateError::Entropy)?;
                if mirror.get(&id).is_some() {
                    continue;
                }
                let record = TokenRecord {
                    id: id.0,
                    owner_user_id: input.owner_user_id,
                    label: input.label.clone(),
                    mac: compute_mac(&key, 1, 1, &id, &secret),
                    key_id: 1,
                    format_version: 1,
                    created_at_ns: now,
                    expires_at_ns: expires,
                    mode: input.mode,
                    channels: input.channels,
                    databases: input.databases.clone(),
                    roles: input.owner_roles.clone(),
                    revoked_at_ns: None,
                };
                record
                    .validate_shape()
                    .map_err(|_| TokenCreateError::InvalidInput)?;
                if store
                    .put_if_absent_blocking(&record)
                    .map_err(TokenCreateError::Store)?
                {
                    generated = Some((record, encode_token(&id, &secret)));
                    break;
                }
            }
            let (record, token) = generated.ok_or(TokenCreateError::IdCollision)?;
            if mirror_insert_conflict.swap(false, std::sync::atomic::Ordering::AcqRel) {
                let mut conflict = record.clone();
                conflict.label.push_str("-conflict");
                let _ = mirror.insert(conflict);
            }
            if !mirror.insert(record.clone()) {
                store
                    .delete_blocking(&TokenId(record.id))
                    .map_err(TokenCreateError::Store)?;
                return Err(TokenCreateError::IdCollision);
            }
            let details = rmp_serde::to_vec_named(&(record.id, record.owner_user_id))
                .map_err(TokenStoreError::Encoding)
                .map_err(TokenCreateError::Store)?;
            if let Err(error) = audit.append(
                "api_token_created",
                audit_context.transport.as_str(),
                audit_context.actor_username.as_str(),
                "",
                audit_context.session_id_prefix,
                "ok",
                details,
                now,
            ) {
                tracing::warn!(error = ?error, "api token audit append failed");
            }
            counters.count_created();
            Ok((record, token))
        })
        .await
        .map_err(|_| TokenCreateError::Store(TokenStoreError::Join))??;
        Ok(CreatedToken {
            record_view: TokenView::from(&result.0),
            token: result.1,
        })
    }

    /// Cancel/close this token's active sessions after durable revocation publishes.
    pub async fn revoke(&self, id: TokenId, now_ns: u64) -> Result<bool, TokenCreateError> {
        self.revoke_with_context(id, now_ns, TokenAuditContext::service_default())
            .await
    }
    /// Revoke a token with explicit audit context.
    pub async fn revoke_with_context(
        &self,
        id: TokenId,
        now_ns: u64,
        audit_context: TokenAuditContext,
    ) -> Result<bool, TokenCreateError> {
        self.revoke_inner(id, now_ns, audit_context).await
    }
    async fn revoke_inner(
        &self,
        id: TokenId,
        now_ns: u64,
        audit_context: TokenAuditContext,
    ) -> Result<bool, TokenCreateError> {
        if self.read_only_node {
            return Err(TokenCreateError::ReadOnlyNode);
        }
        let Some(owner) = self.mirror.get(&id).map(|s| s.record().owner_user_id) else {
            let store = self.store.clone();
            let retained = tokio::task::spawn_blocking(move || store.get_blocking(&id))
                .await
                .map_err(|_| TokenCreateError::Store(TokenStoreError::Join))?
                .map_err(TokenCreateError::Store)?;
            let Some(retained) = retained else {
                return Err(TokenCreateError::NotFound);
            };
            let gate =
                Arc::clone(&self.mutation_gates[usize::from(retained.owner_user_id[0]) % 64])
                    .lock_owned()
                    .await;
            let store = self.store.clone();
            let audit = Arc::clone(&self.audit);
            let counters = Arc::clone(&self.counters);
            return tokio::task::spawn_blocking(move || {
                let _gate = gate;
                let outcome = store.with_locked_records(|db, ks| {
                    let Some(mut current) = store.scan_locked()?.into_iter().find(|r| r.id == id.0)
                    else {
                        return Ok(None);
                    };
                    if current.revoked_at_ns.is_some() {
                        return Ok(Some((current.owner_user_id, false)));
                    }
                    current.revoked_at_ns = Some(now_ns.max(1));
                    store.write_locked(db, ks, &current)?;
                    Ok(Some((current.owner_user_id, true)))
                })?;
                match outcome {
                    None => Ok(None),
                    Some((_, false)) => Ok(Some(false)),
                    Some((owner_id, true)) => {
                        counters.count_revoked();
                        let details = rmp_serde::to_vec_named(&(id.0, owner_id))
                            .map_err(TokenStoreError::Encoding)?;
                        if let Err(error) = audit.append(
                            "api_token_revoked",
                            audit_context.transport.as_str(),
                            audit_context.actor_username.as_str(),
                            "",
                            audit_context.session_id_prefix,
                            "ok",
                            details,
                            now_ns,
                        ) {
                            tracing::warn!(error = ?error, "api token audit append failed");
                        }
                        Ok(Some(true))
                    }
                }
            })
            .await
            .map_err(|_| TokenCreateError::Store(TokenStoreError::Join))?
            .map_err(TokenCreateError::Store)?
            .ok_or(TokenCreateError::NotFound);
        };
        let gate = Arc::clone(&self.mutation_gates[usize::from(owner[0]) % 64])
            .lock_owned()
            .await;
        let store = self.store.clone();
        let mirror = Arc::clone(&self.mirror);
        let sessions = Arc::clone(&self.sessions);
        let canceller = Arc::clone(&self.canceller);
        let audit = Arc::clone(&self.audit);
        let counters = Arc::clone(&self.counters);
        tokio::task::spawn_blocking(move || {
            let _gate = gate;
            let Some(state) = mirror.get(&id) else {
                return Ok(false);
            };
            if state.is_revoked() {
                return Ok(false);
            }
            let mut next = state.record().clone();
            next.revoked_at_ns = Some(now_ns.max(1));
            store.with_locked_records(|db, ks| store.write_locked(db, ks, &next))?;
            mirror.revoke(&id, now_ns);
            counters.count_revoked();
            canceller.cancel_token(&id);
            for session in sessions.sessions_of_token(&id) {
                sessions.remove(&session.session_id);
            }
            let details = rmp_serde::to_vec_named(&(next.id, next.owner_user_id))
                .map_err(TokenStoreError::Encoding)?;
            if let Err(error) = audit.append(
                "api_token_revoked",
                audit_context.transport.as_str(),
                audit_context.actor_username.as_str(),
                "",
                audit_context.session_id_prefix,
                "ok",
                details,
                now_ns,
            ) {
                tracing::warn!(error = ?error, "api token audit append failed");
            }
            Ok(true)
        })
        .await
        .map_err(|_| TokenCreateError::Store(TokenStoreError::Join))?
        .map_err(TokenCreateError::Store)
    }
    /// Revoke all tokens belonging to one owner.
    pub async fn revoke_user(
        &self,
        owner: [u8; 16],
        now_ns: u64,
    ) -> Result<usize, TokenCreateError> {
        self.revoke_user_with_context(owner, now_ns, TokenAuditContext::service_default())
            .await
    }
    /// Revoke all tokens belonging to one owner with explicit audit context.
    pub async fn revoke_user_with_context(
        &self,
        owner: [u8; 16],
        now_ns: u64,
        audit_context: TokenAuditContext,
    ) -> Result<usize, TokenCreateError> {
        if self.read_only_node {
            return Err(TokenCreateError::ReadOnlyNode);
        }
        let gate = Arc::clone(&self.mutation_gates[usize::from(owner[0]) % 64])
            .lock_owned()
            .await;
        let store = self.store.clone();
        let mirror = Arc::clone(&self.mirror);
        let sessions = Arc::clone(&self.sessions);
        let canceller = Arc::clone(&self.canceller);
        let audit = Arc::clone(&self.audit);
        let counters = Arc::clone(&self.counters);
        tokio::task::spawn_blocking(move || {
            let _gate = gate;
            let rows = store.with_locked_records(|db, ks| {
                let mut rows: Vec<_> = store
                    .scan_locked()?
                    .into_iter()
                    .filter(|r| {
                        r.owner_user_id == owner
                            && r.revoked_at_ns.is_none()
                            && r.expires_at_ns > now_ns
                    })
                    .collect();
                for row in &mut rows {
                    row.revoked_at_ns = Some(now_ns.max(1));
                }
                let mut batch = db.batch().durability(Some(fjall::PersistMode::SyncAll));
                for row in &rows {
                    let bytes = rmp_serde::to_vec_named(row).map_err(TokenStoreError::Encoding)?;
                    batch.insert(ks, row.id, bytes.as_slice());
                }
                if !rows.is_empty() {
                    store.fail_locked_write()?;
                    batch.commit()?;
                }
                Ok(rows)
            })?;
            for row in &rows {
                let id = TokenId(row.id);
                mirror.revoke(&id, now_ns);
                canceller.cancel_token(&id);
                for session in sessions.sessions_of_token(&id) {
                    sessions.remove(&session.session_id);
                }
                counters.count_revoked();
            }
            let details =
                rmp_serde::to_vec_named(&(owner, rows.iter().map(|r| r.id).collect::<Vec<_>>()))
                    .map_err(TokenStoreError::Encoding)?;
            if let Err(error) = audit.append(
                "api_token_user_revoked",
                audit_context.transport.as_str(),
                audit_context.actor_username.as_str(),
                "",
                audit_context.session_id_prefix,
                "ok",
                details,
                now_ns,
            ) {
                tracing::warn!(error = ?error, "api token audit append failed");
            }
            Ok(rows.len())
        })
        .await
        .map_err(|_| TokenCreateError::Store(TokenStoreError::Join))?
        .map_err(TokenCreateError::Store)
    }
    /// Find a retained token record by id without exposing its bearer secret.
    pub async fn lookup(&self, id: [u8; 16]) -> Result<Option<TokenView>, TokenStoreError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            Ok(store
                .get_blocking(&TokenId(id))?
                .as_ref()
                .map(TokenView::from))
        })
        .await
        .map_err(|_| TokenStoreError::Join)?
    }

    /// List the owner's secret-free token metadata.
    pub async fn list(&self, owner: [u8; 16]) -> Result<Vec<TokenView>, TokenStoreError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            Ok(store
                .scan_blocking()?
                .iter()
                .filter(|r| r.owner_user_id == owner)
                .map(TokenView::from)
                .collect())
        })
        .await
        .map_err(|_| TokenStoreError::Join)?
    }
    /// Verify a bearer token against the mirror and current owner floor.
    pub fn verify(
        &self,
        header_bytes: &[u8],
        endpoint: Endpoint,
        now_ns: u64,
    ) -> Result<shamir_connect::server::token::Verified, VerifyFailure> {
        let result = verify_token(
            &VerifyCtx {
                mirror: &self.mirror,
                key: &self.token_key,
                key_id: 1,
                owners: &*self.owners,
                now_ns,
                endpoint,
            },
            header_bytes,
        );
        self.counters.count_verified(result.is_ok());
        result
    }
    /// Collect rows beyond expiry/revocation retention; no-op on read-only nodes.
    pub async fn collect_garbage_now(&self) -> Result<usize, TokenStoreError> {
        if self.read_only_node {
            return Ok(0);
        }
        self.clone_for_gc().collect().await
    }
    /// Start hourly GC, stopping with the provided cancellation token.
    pub async fn start_gc(&self, shutdown: CancellationToken) {
        if self.read_only_node {
            return;
        }
        let mut task = self.gc_task.lock().await;
        if task.is_some() {
            return;
        }
        let gc = self.clone_for_gc();
        let stop = self.gc_shutdown.child_token();
        *task = Some(tokio::spawn(async move {
            let _ = gc.collect().await;
            let mut interval = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(3600),
                Duration::from_secs(3600),
            );
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { biased; _ = stop.cancelled() => break, _ = shutdown.cancelled() => break, _ = interval.tick() => { let _ = gc.collect().await; } }
            }
        }));
    }
    pub async fn gc_task_finished(&self) -> bool {
        match self.gc_task.lock().await.as_ref() {
            Some(task) => task.is_finished(),
            None => self.gc_shutdown.is_cancelled(),
        }
    }
    pub async fn shutdown_gc(&self) {
        self.gc_shutdown.cancel();
        let task = self.gc_task.lock().await.take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
    fn clone_for_gc(&self) -> GcService {
        GcService::new(
            self.store.clone(),
            Arc::clone(&self.mirror),
            Arc::clone(&self.mutation_gates),
            Arc::clone(&self.clock),
        )
    }
}
impl Drop for TokenService {
    fn drop(&mut self) {
        self.gc_shutdown.cancel();
    }
}
