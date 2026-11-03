use super::reader::{bin_value, uint_value, Reader, WireError};

/// Borrowed validated FINISH request.
pub struct FinishRequest {
    /// Handshake identifier.
    pub handshake_id: [u8; 32],
    /// Client proof, to be compared in constant time by the service.
    pub client_proof: [u8; 32],
}

impl FinishRequest {
    /// Parse an exact FINISH MessagePack body (maximum 256 bytes).
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() > 256 {
            return Err(WireError::Invalid);
        }
        let mut reader = Reader::new(bytes);
        let pairs = reader.map(&["protocol_version", "handshake_id", "client_proof"])?;
        reader.finish()?;
        let mut version = None;
        let mut handshake_id = None;
        let mut client_proof = None;
        for (key, value) in pairs {
            match key {
                "protocol_version" => version = Some(uint_value(value, 255)?),
                "handshake_id" => handshake_id = Some(bin_value::<32>(value)?),
                "client_proof" => client_proof = Some(bin_value::<32>(value)?),
                _ => return Err(WireError::Invalid),
            }
        }
        if version != Some(1) {
            return Err(WireError::Invalid);
        }
        Ok(Self {
            handshake_id: handshake_id.ok_or(WireError::Invalid)?,
            client_proof: client_proof.ok_or(WireError::Invalid)?,
        })
    }
}
