use std::time::Duration;

use napi::bindgen_prelude::*;

use shamir_client as core;

use crate::{bounded, checked_port};

/// DNS budget when the caller supplied no connect/handshake timeout, so a
/// stalled resolver never hangs a token call forever.
const DEFAULT_RESOLVE_BUDGET: Duration = Duration::from_secs(10);

enum Scheme {
    Plain,
    Https(core::TokenTrust),
}

/// Synchronously validated endpoint inputs; resolving the host is the only
/// asynchronous step left.
pub(crate) struct EndpointSpec {
    host: String,
    port: u16,
    server_name: String,
    scheme: Scheme,
}

impl EndpointSpec {
    pub(crate) fn parse(
        host: String,
        port: u32,
        server_name: Option<String>,
        ca_pem: Option<String>,
        plain_loopback: Option<bool>,
    ) -> Result<Self> {
        let port = checked_port(port)?;
        let scheme = if plain_loopback.unwrap_or(false) {
            if ca_pem.is_some() {
                return Err(Error::from_reason("caPem is not used with plainLoopback"));
            }
            Scheme::Plain
        } else {
            let trust = match ca_pem {
                // The underlying error can echo a line of the PEM input, so it
                // is dropped in favour of a fixed message.
                Some(pem) => core::TokenTrust::pem(pem.as_bytes())
                    .map_err(|_| Error::from_reason("caPem: no valid PEM certificate"))?,
                None => core::TokenTrust::webpki_roots(),
            };
            Scheme::Https(trust)
        };
        let server_name = server_name.unwrap_or_else(|| host.clone());
        Ok(Self {
            host,
            port,
            server_name,
            scheme,
        })
    }

    /// Resolve the first address of the host under `budget` and build the
    /// core endpoint; the core refuses plain HTTP on non-loopback addresses.
    pub(crate) async fn resolve(
        self,
        budget: Option<Duration>,
    ) -> std::result::Result<core::TokenEndpoint, core::ClientError> {
        let budget = Some(budget.unwrap_or(DEFAULT_RESOLVE_BUDGET));
        let addr = bounded(budget, "dns", core::resolve_host(&self.host, self.port)).await?;
        match self.scheme {
            Scheme::Plain => core::TokenEndpoint::plain_loopback(addr),
            Scheme::Https(trust) => Ok(core::TokenEndpoint::https(addr, self.server_name, trust)),
        }
    }
}
