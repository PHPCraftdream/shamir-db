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

    /// R4b-1 — rid-carrying variant of [`Self::handle_decoded`]. The client's
    /// OPTIONAL transport request id is carried alongside the server-minted
    /// execution id; it never replaces it. The default ignores the rid and
    /// delegates to [`Self::handle_decoded`], preserving the old contract for
    /// every implementor that does not track in-flight requests.
    fn handle_decoded_with_request_id<'a>(
        &'a self,
        session: &'a Session,
        request: DbRequest,
        conn: &'a ConnectionServices,
        client_rid: Option<u32>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>> {
        let _ = client_rid;
        self.handle_decoded(session, request, conn)
    }

    fn cleanup_session(&self, sid: &[u8; 32]);
}
