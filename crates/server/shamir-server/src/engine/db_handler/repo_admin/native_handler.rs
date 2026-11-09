use std::future::Future;
use std::pin::Pin;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::Session;

use crate::security::http_rest::NativeDbHandler;

use super::handler::{DbRequest, ShamirDbHandler};

impl NativeDbHandler for ShamirDbHandler {
    fn handle_decoded<'a>(
        &'a self,
        session: &'a Session,
        request: DbRequest,
        conn: &'a ConnectionServices,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>> {
        Box::pin(ShamirDbHandler::handle_decoded(
            self, session, request, conn,
        ))
    }

    fn cleanup_session(&self, sid: &[u8; 32]) {
        drop(self.tx_registry().remove_for_session(sid));
        drop(self.cursor_registry().remove_all_for_session(sid));
    }
}
