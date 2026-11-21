//! Token sessions use only the SDK's verified HTTPS and native token WSS paths.

#[cfg(not(unix))]
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

#[cfg(unix)]
use rustix::fs::{open, Mode, OFlags};
use serde_json::Value;
use shamir_client::{
    ApiToken, Client, ClientError, TokenEndpoint, TokenHttpClient, TokenHttpOptions, TokenTrust,
    TokenWsOptions,
};
use shamir_query_types::batch::{BatchRequest, BatchResponse};
use tokio::time::{timeout_at, Instant};
use zeroize::Zeroizing;

use super::profile::{Profile, Transport};
use super::scram::{request_error, REQUEST_TIMEOUT};
use crate::shell::output::ShellError;

const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TOTAL: Duration = Duration::from_secs(30);
const MAX_CA_BYTES: u64 = 1024 * 1024;

/// Shell-only dispatch; the parser gates commands before using this client.
pub(in crate::shell) enum SessionClient {
    Scram(Client),
    TokenWs(GuardedTokenClient<Client>),
    TokenRest(Box<GuardedTokenClient<TokenHttpClient>>),
}

/// Keep the credential private and wipe it when the session is dropped.
pub(in crate::shell) struct GuardedTokenClient<T> {
    credential: Zeroizing<String>,
    client: T,
}

impl SessionClient {
    pub(in crate::shell) fn is_token(&self) -> bool {
        match self {
            Self::Scram(_) => false,
            Self::TokenWs(_) | Self::TokenRest(_) => true,
        }
    }

    /// TLS authenticates the endpoint, not the catalogue text it returns.
    pub(in crate::shell) fn check_catalogue_output(
        &self,
        projected: &Value,
    ) -> Result<(), ShellError> {
        let token = match self {
            Self::Scram(_) => return Ok(()),
            Self::TokenWs(session) => session.credential.as_str(),
            Self::TokenRest(session) => session.credential.as_str(),
        };
        // ApiToken validated the canonical shk1.<id>.<secret> format.
        let (_, suffix) = token.rsplit_once('.').ok_or(ShellError::ProjectionLimit)?;
        let reflects = |text: &str| text.contains(token) || text.contains(suffix);
        if contains_credential(projected, &reflects) {
            return Err(ShellError::ProjectionLimit);
        }
        Ok(())
    }

    /// cancel-safe: NO — a cancelled Ping may remain in flight; do not retry.
    pub(in crate::shell) async fn ping(&self) -> Result<(), ClientError> {
        match self {
            Self::Scram(client) => client.ping().await,
            Self::TokenWs(session) => session.client.ping().await,
            Self::TokenRest(session) => session.client.ping().await,
        }
    }

    /// cancel-safe: NO — a cancelled command may remain in flight; do not retry.
    pub(in crate::shell) async fn execute(
        &self,
        db: &str,
        batch: BatchRequest,
    ) -> Result<BatchResponse, ClientError> {
        match self {
            Self::Scram(client) => client.execute(db, batch).await,
            Self::TokenWs(session) => session.client.execute(db, batch).await,
            Self::TokenRest(session) => session.client.execute(db, batch).await,
        }
    }

    /// Authenticates an explicit REST `connect`. Batch commands should send
    /// only their own request, and WSS has already authenticated at upgrade.
    /// cancel-safe: NO — a cancelled Ping may remain in flight; drop the session.
    pub(in crate::shell) async fn verify_rest(&self) -> Result<(), ClientError> {
        match self {
            Self::TokenRest(session) => session.client.ping().await,
            Self::Scram(_) | Self::TokenWs(_) => Ok(()),
        }
    }
}

// Only called on catalogue JSON already depth-bounded by output::value.
fn contains_credential(value: &Value, reflects: &impl Fn(&str) -> bool) -> bool {
    match value {
        Value::String(text) => reflects(text),
        Value::Array(items) => items.iter().any(|item| contains_credential(item, reflects)),
        Value::Object(fields) => fields
            .iter()
            .any(|(key, item)| reflects(key) || contains_credential(item, reflects)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// A prepared token endpoint; no bearer material is stored in the profile.
pub(in crate::shell) struct TokenConnection {
    wss: bool,
    host: String,
    port: u16,
    sni: Option<String>,
    trust: TokenTrust,
}

/// Validate auth/transport compatibility before reading any secret or dialing.
pub(in crate::shell) fn prepare_token(profile: &Profile) -> Result<TokenConnection, ShellError> {
    let wss = match profile.transport_kind()? {
        Transport::TokenRest => false,
        Transport::TokenWss => true,
        Transport::TlsTcp | Transport::Ipc | Transport::NativeWss | Transport::Plaintext => {
            return Err(ShellError::Token);
        }
    };
    // Token identity comes solely from the bearer credential. A username in
    // a shared profile is not forwarded; an application pin cannot set trust.
    if profile.pin.is_some() {
        return Err(ShellError::InvalidProfile);
    }
    let (host, port) = profile.tls_address()?;
    let trust = match profile.ca_path.as_deref() {
        Some(path) => read_ca(path)?,
        None => TokenTrust::webpki_roots(),
    };
    Ok(TokenConnection {
        wss,
        host,
        port,
        sni: profile.sni.clone(),
        trust,
    })
}

impl TokenConnection {
    /// cancel-safe: NO — an upgrade may have reached the peer; abandon the dial.
    pub(in crate::shell) async fn connect(
        self,
        secret: Zeroizing<Vec<u8>>,
    ) -> Result<SessionClient, ShellError> {
        let text = std::str::from_utf8(&secret).map_err(|_| ShellError::Authentication)?;
        // ApiToken::new takes ownership of the String in order to zeroize it.
        let token = ApiToken::new(text.to_owned()).map_err(request_error)?;
        let credential = Zeroizing::new(text.to_owned());
        drop(secret);

        let start = Instant::now();
        let dial_deadline = start + DIAL_TIMEOUT;
        let addr = timeout_at(
            dial_deadline,
            tokio::net::lookup_host((self.host.as_str(), self.port)),
        )
        .await
        .map_err(|_| ShellError::Connection)?
        .map_err(|_| ShellError::Connection)?
        .next()
        .ok_or(ShellError::Connection)?;
        let endpoint = TokenEndpoint::https(addr, self.sni.unwrap_or(self.host), self.trust);
        let dial_remaining = Some(dial_deadline.saturating_duration_since(Instant::now()));
        if self.wss {
            let opts = TokenWsOptions::new(endpoint, token)
                .with_connect_timeout(dial_remaining)
                .with_request_timeout(Some(REQUEST_TIMEOUT));
            timeout_at(start + CONNECT_TOTAL, Client::connect_token_ws(opts))
                .await
                .map_err(|_| ShellError::Connection)?
                .map(|client| SessionClient::TokenWs(GuardedTokenClient { credential, client }))
                .map_err(request_error)
        } else {
            let opts = TokenHttpOptions::new(endpoint, token)
                .with_connect_timeout(dial_remaining)
                .with_request_timeout(Some(REQUEST_TIMEOUT));
            timeout_at(dial_deadline, TokenHttpClient::connect_once(opts))
                .await
                .map_err(|_| ShellError::Connection)?
                .map(|client| {
                    SessionClient::TokenRest(Box::new(GuardedTokenClient { credential, client }))
                })
                .map_err(request_error)
        }
    }
}

fn read_ca(path: &Path) -> Result<TokenTrust, ShellError> {
    // The operator chooses this trust source in a verified profile. Reject
    // special files, and check the opened handle as well as capping the read.
    let file = open_ca(path)?;
    if !file
        .metadata()
        .map_err(|_| ShellError::InvalidProfile)?
        .is_file()
    {
        return Err(ShellError::InvalidProfile);
    }
    let mut pem = Zeroizing::new(Vec::with_capacity(MAX_CA_BYTES as usize + 1));
    file.take(MAX_CA_BYTES + 1)
        .read_to_end(&mut pem)
        .map_err(|_| ShellError::InvalidProfile)?;
    if pem.len() as u64 > MAX_CA_BYTES {
        return Err(ShellError::InvalidProfile);
    }
    TokenTrust::pem(&pem).map_err(|_| ShellError::InvalidProfile)
}

#[cfg(unix)]
fn open_ca(path: &Path) -> Result<File, ShellError> {
    // Nonblocking open lets the handle check reject FIFOs without waiting for
    // a writer, including if the selected path is replaced before open.
    open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| ShellError::InvalidProfile)
}

#[cfg(not(unix))]
fn open_ca(path: &Path) -> Result<File, ShellError> {
    if !fs::metadata(path)
        .map_err(|_| ShellError::InvalidProfile)?
        .is_file()
    {
        return Err(ShellError::InvalidProfile);
    }
    File::open(path).map_err(|_| ShellError::InvalidProfile)
}
