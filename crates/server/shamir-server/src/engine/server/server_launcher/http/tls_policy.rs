//! Focused policy validation for HTTP TLS certificates.

use std::net::SocketAddr;
use std::sync::Arc;

use hex::encode as hex_encode;
use rustls::pki_types::pem::PemObject;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::boot::config::TlsConfig;
use crate::server::boot_error::BootError;

/// Validates the first certificate from a single PEM snapshot and builds the
/// server config from that same snapshot and its paired private-key snapshot.
///
/// Certificates issued by a distinct issuer are accepted. Self-issued leaf
/// certificates require explicit permission on remote listeners; permitted
/// remote use logs a SHA-256 pin of the complete SubjectPublicKeyInfo DER.
pub(in crate::engine::server::server_launcher) fn validate_http_tls(
    config: &TlsConfig,
    address: SocketAddr,
    allow_self_signed: bool,
) -> Result<Arc<rustls::ServerConfig>, BootError> {
    let pem = std::fs::read_to_string(&config.cert_path)
        .map_err(|error| BootError::Bind(format!("HTTP TLS certificate read: {error}")))?;
    let cert = rustls::pki_types::CertificateDer::pem_slice_iter(pem.as_bytes())
        .next()
        .ok_or_else(|| BootError::Bind("HTTP TLS certificate PEM contains no certificate".into()))?
        .map_err(|error| BootError::Bind(format!("HTTP TLS certificate PEM: {error}")))?;
    let (issuer, subject, spki) = certificate_fields(cert.as_ref()).ok_or_else(|| {
        BootError::Bind("HTTP TLS certificate has malformed DER structure".into())
    })?;
    if issuer == subject && !address.ip().is_loopback() && !allow_self_signed {
        return Err(BootError::Bind(
            "remote HTTP TLS listener refuses self-issued certificate; set allow_self_signed"
                .into(),
        ));
    }
    if issuer == subject && !address.ip().is_loopback() {
        let pin = hex_encode(Sha256::digest(spki));
        tracing::warn!(%address, spki_sha256 = %pin, "HTTP TLS self-issued certificate explicitly allowed");
    }
    let key_pem = Zeroizing::new(
        std::fs::read_to_string(&config.key_path)
            .map_err(|error| BootError::Bind(format!("HTTP TLS key read: {error}")))?,
    );
    shamir_transport_tcp::tls::make_server_config_from_pem(&pem, key_pem.as_str())
        .map_err(|error| BootError::Bind(format!("HTTP TLS config: {error}")))
}

fn certificate_fields(der: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let (tag, _, cert_value, rest) = read_tlv(der)?;
    if tag != 0x30 || !rest.is_empty() {
        return None;
    }
    let (tag, _, tbs, _) = read_tlv(cert_value)?;
    if tag != 0x30 {
        return None;
    }
    let mut fields = tbs;
    if fields.first() == Some(&0xa0) {
        let (_, _, _, remaining) = read_tlv(fields)?;
        fields = remaining;
    }
    let (_, _, _, fields) = read_tlv(fields)?; // serialNumber
    let (tag, _, _, fields) = read_tlv(fields)?; // signature
    if tag != 0x30 {
        return None;
    }
    let (tag, issuer, _, fields) = read_tlv(fields)?;
    if tag != 0x30 {
        return None;
    }
    let (_, _, _, fields) = read_tlv(fields)?; // validity
    let (tag, subject, _, fields) = read_tlv(fields)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, spki, _, _) = read_tlv(fields)?;
    if tag != 0x30 {
        return None;
    }
    Some((issuer, subject, spki))
}

type Tlv<'a> = (u8, &'a [u8], &'a [u8], &'a [u8]);

/// Returns tag, complete TLV, content, and unconsumed input.
fn read_tlv(input: &[u8]) -> Option<Tlv<'_>> {
    let tag = *input.first()?;
    let first_len = *input.get(1)?;
    let (length, header) = if first_len & 0x80 == 0 {
        (usize::from(first_len), 2usize)
    } else {
        let count = usize::from(first_len & 0x7f);
        if count == 0 || count > std::mem::size_of::<usize>() {
            return None;
        }
        let length_bytes = input.get(2..2usize.checked_add(count)?)?;
        if length_bytes.first() == Some(&0) {
            return None;
        }
        let mut length = 0usize;
        for byte in length_bytes {
            length = length.checked_mul(256)?.checked_add(usize::from(*byte))?;
        }
        if length < 128 {
            return None;
        }
        (length, 2usize.checked_add(count)?)
    };
    let end = header.checked_add(length)?;
    let content = input.get(header..end)?;
    let rest = input.get(end..)?;
    Some((tag, input.get(..end)?, content, rest))
}

#[cfg(test)]
#[path = "tests/tls_policy_tests.rs"]
mod tests;
