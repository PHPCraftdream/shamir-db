use std::sync::Arc;

use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

use crate::error::ClientError;

/// Which certificate authorities the token client trusts.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum TokenTrust {
    /// The bundled Mozilla root set.
    WebPkiRoots,
    /// Caller-supplied DER certificates.
    CustomRoots(Vec<Vec<u8>>),
}

impl TokenTrust {
    pub fn webpki_roots() -> Self {
        Self::WebPkiRoots
    }

    /// Parse every certificate in `pem`; at least one is required.
    pub fn pem(pem: &[u8]) -> Result<Self, ClientError> {
        let certs = CertificateDer::pem_slice_iter(pem)
            .map(|c| c.map(|c| c.as_ref().to_vec()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ClientError::tls("pem", e))?;
        if certs.is_empty() {
            return Err(ClientError::tls("pem", "no certificate found"));
        }
        Ok(Self::CustomRoots(certs))
    }

    pub fn der(cert: impl Into<Vec<u8>>) -> Self {
        Self::CustomRoots(vec![cert.into()])
    }
}

pub(crate) fn build_connector(trust: &TokenTrust) -> Result<TlsConnector, ClientError> {
    let roots = match trust {
        TokenTrust::WebPkiRoots => {
            RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned())
        }
        TokenTrust::CustomRoots(ders) => {
            let mut store = RootCertStore::empty();
            let mut accepted = 0usize;
            for der in ders {
                store
                    .add(CertificateDer::from(der.clone()))
                    .map_err(|e| ClientError::tls("root certificate", e))?;
                accepted += 1;
            }
            if accepted == 0 {
                return Err(ClientError::tls("root certificate", "no roots supplied"));
            }
            store
        }
    };
    let mut config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config.enable_early_data = false;
    Ok(TlsConnector::from(Arc::new(config)))
}

pub(crate) fn parse_server_name(name: &str) -> Result<ServerName<'static>, ClientError> {
    ServerName::try_from(name.to_owned()).map_err(|e| ClientError::tls("server name", e))
}
