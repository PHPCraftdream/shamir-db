//! ShamirDB TLS server — MessagePack + length-prefixed framing.

use std::net::SocketAddr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::framing::{read_msg, write_msg, FrameError};
use crate::db::ShamirDb;

/// Server configuration.
pub struct ServerConfig {
    pub addr: SocketAddr,
    pub db_name: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: "127.0.0.1:3742".parse().unwrap(),
            db_name: "default".to_string(),
        }
    }
}

/// Auth request from client.
#[derive(Debug, Deserialize)]
pub struct AuthRequest {
    pub user: String,
    pub password: String,
}

/// Auth response to client.
#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Run the ShamirDB server.
pub async fn run_server(
    shamir: Arc<ShamirDb>,
    config: ServerConfig,
    acceptor: TlsAcceptor,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&config.addr).await?;
    log::info!("ShamirDB listening on {}", config.addr);

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let shamir = Arc::clone(&shamir);
        let db_name = config.db_name.clone();

        tokio::spawn(async move {
            match acceptor.accept(stream).await {
                Ok(tls_stream) => {
                    log::info!("TLS connection from {}", peer_addr);
                    if let Err(e) = handle_connection(tls_stream, shamir, db_name).await {
                        log::debug!("Connection closed from {}: {}", peer_addr, e);
                    }
                }
                Err(e) => {
                    log::error!("TLS handshake failed from {}: {}", peer_addr, e);
                }
            }
        });
    }
}

async fn handle_connection(
    stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    shamir: Arc<ShamirDb>,
    db_name: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut reader, mut writer) = tokio::io::split(stream);

    // Phase 1: Authentication
    let auth_req: AuthRequest = read_msg(&mut reader).await?;

    let session_id = match authenticate(&auth_req, &shamir).await {
        Ok(sid) => {
            write_msg(&mut writer, &AuthResponse {
                authenticated: true,
                session_id: Some(sid.clone()),
                error: None,
            }).await?;
            sid
        }
        Err(e) => {
            write_msg(&mut writer, &AuthResponse {
                authenticated: false,
                session_id: None,
                error: Some("authentication_failed".to_string()),
            }).await?;
            return Err(e);
        }
    };

    log::info!("Authenticated session: {}", session_id);

    // Phase 2: Query loop — msgpack BatchRequest → BatchResponse
    loop {
        let request: crate::db::query::batch::BatchRequest = match read_msg(&mut reader).await {
            Ok(req) => req,
            Err(FrameError::ConnectionClosed) => break,
            Err(e) => {
                log::error!("Frame error: {}", e);
                break;
            }
        };

        let response = match shamir.execute(&db_name, &request).await {
            Ok(resp) => resp,
            Err(e) => {
                // Send error as BatchResponse with empty results
                let error_resp = serde_json::json!({
                    "id": request.id,
                    "error": e.to_string()
                });
                write_msg(&mut writer, &error_resp).await?;
                continue;
            }
        };

        write_msg(&mut writer, &response).await?;
    }

    Ok(())
}

async fn authenticate(
    req: &AuthRequest,
    shamir: &ShamirDb,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let users_table = shamir.system_store().users_table().await
        .map_err(|e| format!("SystemStore error: {}", e))?;
    let interner = users_table.interner().get().await
        .map_err(|e| format!("Interner error: {}", e))?;
    let refs = crate::types::common::new_map();
    let ctx = crate::db::query::filter::FilterContext::new(interner, &refs);

    let query = crate::db::query::read::ReadQuery::new("users")
        .filter(crate::db::query::filter::Filter::Eq {
            field: vec!["name".to_string()],
            value: crate::db::query::filter::FilterValue::String(req.user.clone()),
        });

    let result = users_table.read(&query, &ctx).await
        .map_err(|e| format!("Query error: {}", e))?;

    if result.records.is_empty() {
        return Err("authentication_failed".into());
    }

    let user = &result.records[0];
    let stored_hash = user.get("password_hash")
        .and_then(|v| v.as_str())
        .ok_or("authentication_failed")?;

    // Simple password check (TODO: SCRAM)
    if stored_hash != req.password {
        return Err("authentication_failed".into());
    }

    let session_id = format!("sess_{:016x}", rand::random::<u64>());
    Ok(session_id)
}
