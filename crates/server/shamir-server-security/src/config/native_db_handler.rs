use std::future::Future;
use std::pin::Pin;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::Session;
use shamir_query_types::wire::DbRequest;

/// Open internal workspace contract for native database dispatch and session cleanup.
pub trait NativeDbHandler: Send + Sync {
    fn handle_decoded<'a>(
        &'a self,
        session: &'a Session,
        request: DbRequest,
        conn: &'a ConnectionServices,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>>;

    fn cleanup_session(&self, sid: &[u8; 32]);
}
