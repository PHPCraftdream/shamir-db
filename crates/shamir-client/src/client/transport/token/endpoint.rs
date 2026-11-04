use std::net::SocketAddr;

use super::tls::TokenTrust;
use crate::error::ClientError;

/// How the token client reaches the server.
#[derive(Clone, Debug)]
pub(crate) enum TokenScheme {
    Https(TokenTrust),
    PlainLoopback,
}

/// Where and how to reach the REST token endpoint.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct TokenEndpoint {
    addr: SocketAddr,
    server_name: String,
    scheme: TokenScheme,
}

impl TokenEndpoint {
    /// HTTPS endpoint; `server_name` is used for SNI and certificate checks.
    pub fn https(addr: SocketAddr, server_name: impl Into<String>, trust: TokenTrust) -> Self {
        Self {
            addr,
            server_name: server_name.into(),
            scheme: TokenScheme::Https(trust),
        }
    }

    /// Plain HTTP endpoint; refused unless `addr` is a loopback address.
    pub fn plain_loopback(addr: SocketAddr) -> Result<Self, ClientError> {
        if !addr.ip().to_canonical().is_loopback() {
            return Err(ClientError::handshake("plain HTTP is loopback-only"));
        }
        Ok(Self {
            addr,
            server_name: addr.ip().to_string(),
            scheme: TokenScheme::PlainLoopback,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub(crate) fn scheme(&self) -> &TokenScheme {
        &self.scheme
    }

    pub(crate) fn host_header(&self) -> String {
        match &self.scheme {
            TokenScheme::Https(_) if self.addr.port() == 443 => self.server_name.clone(),
            TokenScheme::Https(_) => format!("{}:{}", self.server_name, self.addr.port()),
            TokenScheme::PlainLoopback => self.addr.to_string(),
        }
    }
}
