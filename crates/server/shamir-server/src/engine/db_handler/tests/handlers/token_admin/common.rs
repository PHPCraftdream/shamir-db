//! Shared fixtures and request helpers for token-administration handler regressions.

use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_connect::server::token::OwnerFloors;
use shamir_connect::server::TokenSessionStore;
use shamir_db::ShamirDb;
use shamir_query_types::hmac;
use shamir_query_types::wire::{DbRequest, DbResponse};
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::boot::config::TokensConfig;
use crate::db_handler::{AdminGlue, ShamirDbHandler};
use crate::security::tokens::{DirectoryOwnerFloors, TokenService, TokenServiceOptions};
use crate::user_directory::FjallUserDirectory;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;

pub(crate) const NOW: u64 = 1_800_000_000_000_000_000;
pub(crate) const OWNER: &str = "alice";
pub(crate) const OTHER: &str = "bob";

pub(crate) struct NoopAppender {
    pub(crate) entries: std::sync::Mutex<Vec<AuditEntry>>,
}
impl AuditAppender for NoopAppender {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.entries
            .lock()
            .expect("audit appender entries")
            .push(entry.clone());
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

pub(crate) struct Fixture {
    pub(crate) directory: Arc<FjallUserDirectory>,
    pub(crate) handler: ShamirDbHandler,
    pub(crate) read_only_handler: ShamirDbHandler,
    pub(crate) service: Arc<TokenService>,
    pub(crate) owner: Session,
    pub(crate) other: Session,
    pub(crate) admin: Session,
    pub(crate) audit: Arc<NoopAppender>,
    // Dropped last: the stores above must close before their directory goes.
    pub(crate) _temp: TempDir,
}

impl Fixture {
    pub(crate) async fn new(config: TokensConfig, allow_self_service: bool) -> Self {
        let temp = TempDir::new().expect("tempdir");
        let directory =
            Arc::new(FjallUserDirectory::open(temp.path().join("users")).expect("user directory"));
        let owner_id = directory
            .insert(OWNER.into(), user_record())
            .expect("owner insert");
        let other_id = directory
            .insert(OTHER.into(), user_record())
            .expect("other insert");
        directory
            .grant_role(OWNER, "reader", 0)
            .expect("owner role");
        directory
            .grant_role(OTHER, "reader", 0)
            .expect("other role");
        directory
            .set_superuser(OWNER, true, 0)
            .expect("owner superuser");
        // Alice's ordinary SCRAM session is created explicitly below; this one is the admin.
        let owners: Arc<dyn OwnerFloors + Send + Sync> =
            Arc::new(DirectoryOwnerFloors::new(Arc::clone(&directory)));
        let appender = Arc::new(NoopAppender {
            entries: std::sync::Mutex::new(Vec::new()),
        });
        let audit = Arc::new(AuditChainWriter::new(
            AuditChain::new([17; 32]),
            appender.clone(),
        ));
        let service = Arc::new(
            TokenService::open_with_clock(TokenServiceOptions {
                data_dir: temp.path().join("tokens"),
                token_key: Zeroizing::new([31; 32]),
                owners,
                sessions: Arc::new(TokenSessionStore::new()),
                audit,
                config,
                read_only_node: false,
                clock: Arc::new(|| Some(NOW)),
            })
            .await
            .expect("token service"),
        );
        let db = Arc::new(ShamirDb::init_memory().await.expect("memory db"));
        let handler = ShamirDbHandler::with_admin(
            db,
            AdminGlue {
                user_dir: Arc::clone(&directory),
                kdf: shamir_connect::common::kdf_params::KdfParams::DEFAULT,
                tables_registry: None,
            },
        )
        .with_token_service(Arc::clone(&service), allow_self_service);
        let read_only_handler = handler
            .clone()
            .with_node_mode(crate::db_handler::config::NodeMode::ReadOnly);
        let owner = session(owner_id, OWNER, false, [7; 32], BindingMode::TlsExporter);
        let other = session(other_id, OTHER, false, [8; 32], BindingMode::TlsExporter);
        let admin = session(owner_id, OWNER, true, [9; 32], BindingMode::TlsExporter);
        Self {
            directory,
            handler,
            read_only_handler,
            service,
            owner,
            other,
            admin,
            audit: appender,
            _temp: temp,
        }
    }

    pub(crate) async fn call(&self, session: &Session, request: &DbRequest) -> DbResponse {
        self.call_with(&self.handler, session, request).await
    }

    pub(crate) async fn call_with(
        &self,
        handler: &ShamirDbHandler,
        session: &Session,
        request: &DbRequest,
    ) -> DbResponse {
        let bytes = rmp_serde::to_vec_named(request).expect("encode request");
        let reply = handler
            .handle(session, &bytes, &ConnectionServices::without_push(0))
            .await
            .expect("handler response");
        rmp_serde::from_slice(&reply).expect("decode response")
    }

    pub(crate) fn sign(session: &Session, request: &mut DbRequest) {
        let canonical = hmac::canonical_db_request(request).expect("canonical token request");
        let tag = hmac::compute_tag_hex(&session.hmac_key(), &canonical);
        match request {
            DbRequest::CreateApiToken { hmac, .. }
            | DbRequest::ListApiTokens { hmac, .. }
            | DbRequest::RevokeApiToken { hmac, .. }
            | DbRequest::RevokeUserTokens { hmac, .. } => *hmac = Some(tag),
            _ => panic!("not a token admin request"),
        }
    }

    pub(crate) fn assert_error(response: DbResponse, expected: &str) {
        assert!(
            matches!(&response, DbResponse::Error { code, .. } if code == expected),
            "expected {expected}: {response:?}"
        );
    }
}

pub(crate) fn session(
    user_id: [u8; 16],
    name: &str,
    superuser: bool,
    sid: [u8; 32],
    binding: BindingMode,
) -> Session {
    let mut session = Session::new(
        user_id,
        name.into(),
        SessionPermissions::new(superuser, false, vec!["reader".into()]),
        TransportKind::Tcp,
        binding,
        [0; 32],
        NOW - 5,
    );
    session.session_id = sid;
    session
}

pub(crate) fn user_record() -> UserRecord {
    UserRecord {
        salt: [4; 16],
        stored_key: shamir_connect::common::crypto::StoredKey([5; 32]),
        server_key: Zeroizing::new([6; 32]),
        kdf_params: shamir_connect::common::kdf_params::KdfParams::DEFAULT,
        tickets_invalid_before_ns: 0,
    }
}

pub(crate) fn create(label: &str) -> DbRequest {
    DbRequest::CreateApiToken {
        label: label.into(),
        ttl_secs: None,
        mode: None,
        channels: None,
        databases: None,
        for_user: None,
        hmac: None,
    }
}

pub(crate) fn bearer(response: DbResponse) -> (String, String) {
    let DbResponse::ApiTokenCreated(created) = response else {
        panic!("expected created response")
    };
    (created.id, created.token.reveal().to_owned())
}
