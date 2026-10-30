use std::net::SocketAddr;

use serde::{Deserialize, Deserializer};
use shamir_collections::TFxMap;

use super::prod_factory::parse_upstream_addr;

/// Follower-side replication settings.
#[derive(Debug, Clone, Deserialize)]
pub struct ReplicationConfig {
    /// Stable follower identity advertised to leaders.
    #[serde(default)]
    pub node_id: Option<String>,
    /// Username of the replicator account.
    #[serde(default)]
    pub replicator_user: Option<String>,
    /// Password for the replicator account.
    #[serde(default)]
    pub replicator_password: Option<crate::replication::ReplicatorPassword>,
    /// Decode replicated Put payloads before allocating a version.
    #[serde(default)]
    pub validate_replicated_payload: bool,
    /// SNI hostname to present to the upstream TLS layer.
    #[serde(default = "default_repl_server_name")]
    pub server_name: String,
    /// Approved SHA-256 public-key pins per upstream; absent entries use process-local TOFU.
    #[serde(default)]
    pub upstream_pins: UpstreamPins,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            node_id: None,
            replicator_user: None,
            replicator_password: None,
            validate_replicated_payload: false,
            server_name: default_repl_server_name(),
            upstream_pins: UpstreamPins::default(),
        }
    }
}

fn default_repl_server_name() -> String {
    "localhost".to_string()
}

/// Validated, normalized upstream addresses and their approved public-key digests.
#[derive(Debug, Clone, Default)]
pub struct UpstreamPins(pub(crate) TFxMap<SocketAddr, Vec<[u8; 32]>>);

impl<'de> Deserialize<'de> for UpstreamPins {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = TFxMap::<String, Vec<String>>::deserialize(deserializer)?;
        let mut pins = TFxMap::default();
        for (upstream, values) in raw {
            let addr = parse_upstream_addr(&upstream).map_err(serde::de::Error::custom)?;
            if values.is_empty() {
                return Err(serde::de::Error::custom(format!(
                    "upstream {upstream}: pin list must not be empty"
                )));
            }
            let mut decoded = Vec::with_capacity(values.len());
            for value in values {
                let mut pin = [0u8; 32];
                hex::decode_to_slice(&value, &mut pin).map_err(serde::de::Error::custom)?;
                decoded.push(pin);
            }
            if pins.insert(addr, decoded).is_some() {
                return Err(serde::de::Error::custom(format!(
                    "duplicate normalized upstream {addr}"
                )));
            }
        }
        Ok(Self(pins))
    }
}
