use super::super::LoginServiceContext;
use crate::config::TokensConfig;
use crate::http_login::admission::{AdmissionState, LoginClock};
use crate::http_login::pending::PendingStore;
use crate::security::tokens::{TokenService, TokenServiceOptions};
use crate::security::user_directory::FjallUserDirectory;
use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::lockout::InMemoryLockoutStore;
use shamir_connect::server::token::OwnerFloors;
use shamir_connect::server::user_record::UserRecord;
use shamir_connect::server::TokenSessionStore;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;
use zeroize::Zeroizing;

pub(super) struct Fixture {
    pub(super) _temp: TempDir,
    pub(super) context: LoginServiceContext,
    pub(super) lockout: Arc<InMemoryLockoutStore>,
    pub(super) test_clock: Arc<TestClock>,
}

struct Owners;
impl OwnerFloors for Owners {
    fn floor_if_present(&self, _: &[u8; 16]) -> Option<u64> {
        Some(0)
    }
}

struct NoopAppender;
impl AuditAppender for NoopAppender {
    fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
        Ok(())
    }

    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

struct FixedClock;
impl LoginClock for FixedClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub(super) struct TestClock(std::sync::Mutex<Instant>);
impl TestClock {
    pub(super) fn advance(&self, duration: std::time::Duration) {
        let mut now = self.0.lock().expect("test clock lock");
        *now += duration;
    }

    fn now(&self) -> Instant {
        *self.0.lock().expect("test clock lock")
    }
}
impl LoginClock for TestClock {
    fn now(&self) -> Instant {
        self.now()
    }
}

impl Fixture {
    pub(super) async fn new() -> Self {
        Self::with_options(TokensConfig::default(), false).await
    }

    pub(super) async fn new_with_lockout(lockout: Arc<InMemoryLockoutStore>) -> Self {
        let mut fixture = Self::new().await;
        fixture.context.lockout = Arc::clone(&lockout);
        fixture.lockout = lockout;
        fixture
    }

    pub(super) async fn new_read_only() -> Self {
        Self::with_options(TokensConfig::default(), true).await
    }

    pub(super) async fn with_config(config: TokensConfig) -> Self {
        Self::with_options(config, false).await
    }

    async fn with_options(config: TokensConfig, read_only_node: bool) -> Self {
        Self::with_token_audit_and_options(config, read_only_node, |_| Arc::new(NoopAppender)).await
    }
    pub(super) async fn with_token_audit<F>(make_appender: F) -> Self
    where
        F: FnOnce(Arc<FjallUserDirectory>) -> Arc<dyn AuditAppender>,
    {
        Self::with_token_audit_and_options(TokensConfig::default(), false, make_appender).await
    }

    async fn with_token_audit_and_options<F>(
        config: TokensConfig,
        read_only_node: bool,
        make_appender: F,
    ) -> Self
    where
        F: FnOnce(Arc<FjallUserDirectory>) -> Arc<dyn AuditAppender>,
    {
        let temp = TempDir::new().expect("temporary fixture directory");
        let users =
            Arc::new(FjallUserDirectory::open(temp.path().join("users")).expect("user directory"));
        users
            .insert(
                "alice".to_owned(),
                UserRecord {
                    salt: [0x42; 16],
                    stored_key: StoredKey([0x31; 32]),
                    server_key: Zeroizing::new([0x52; 32]),
                    kdf_params: KdfParams {
                        memory_kb: 19_456,
                        time: 2,
                        parallelism: 1,
                        argon2_version: 0x13,
                    },
                    tickets_invalid_before_ns: 0,
                },
            )
            .expect("seed login user");
        let audit = Arc::new(AuditChainWriter::new(
            AuditChain::new([8; 32]),
            make_appender(Arc::clone(&users)),
        ));
        let sessions = Arc::new(TokenSessionStore::new());
        let tokens = Arc::new(
            TokenService::open_with_clock(TokenServiceOptions {
                data_dir: temp.path().join("tokens"),
                token_key: Zeroizing::new([5; 32]),
                owners: Arc::new(Owners),
                sessions,
                audit,
                config: config.clone(),
                read_only_node,
                clock: Arc::new(|| Some(90_000_000_000)),
            })
            .await
            .expect("token service"),
        );
        let lockout = Arc::new(InMemoryLockoutStore::new());
        let test_clock = Arc::new(TestClock(std::sync::Mutex::new(Instant::now())));
        let context = LoginServiceContext::new(super::super::LoginServiceOptions {
            users,
            tokens,
            lockout: Arc::clone(&lockout),
            secrets: Arc::new(ServerSecrets {
                server_secret: [7; 32],
                lockout_secret: [9; 32],
            }),
            default_kdf: KdfParams {
                memory_kb: 19_456,
                time: 2,
                parallelism: 1,
                argon2_version: 0x13,
            },
            hardening: false,
            security_class: 0,
            listener_id: [3; 16],
            pending: Arc::new(PendingStore::new(test_clock.clone())),
            admission: Arc::new(AdmissionState::new(Arc::new(FixedClock))),
            token_config: config,
            read_only: read_only_node,
            unix_clock: Arc::new(|| Some(90_000_000_000)),
            audit: None,
        })
        .expect("login context");
        Self {
            _temp: temp,
            context,
            lockout,
            test_clock,
        }
    }
}
