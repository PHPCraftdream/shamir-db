//! Ktav-based configuration schema for the ShamirDB v1 production server.
//!
//! The server is driven entirely by a single `shamir-server.ktav` file —
//! data dir, logging level, KDF parameters, listener bindings, TLS cert/key
//! paths — all loaded through [`Config::from_file`] and then sanity-checked
//! by [`Config::validate`] before any sockets are bound.
//!
//! # Schema
//!
//! ```ktav
//! data_dir: /var/lib/shamir-db
//!
//! logging: {
//!     level: info
//! }
//!
//! kdf_defaults: {
//!     memory_kb: 131072    # 128 MB (spec §3.7 default)
//!     time: 4
//!     parallelism: 1
//!     argon2_version: 19   # 0x13
//! }
//!
//! argon2_concurrent_max: 64
//!
//! listeners: [
//!     {
//!         kind: tcp
//!         addr: 0.0.0.0:7331
//!         profile: tls_exporter
//!     }
//!     {
//!         kind: ws
//!         addr: 0.0.0.0:7333
//!         profile: tls_no_export
//!         path: /shamir/v1/browser
//!         browser_origin_allowlist: [
//!             https://app.example.com
//!         ]
//!     }
//! ]
//!
//! tls: {
//!     cert_path: /var/lib/shamir-db/cert.pem
//!     key_path:  /var/lib/shamir-db/key.pem
//! }
//! ```
//!
//! # Validation rules (per spec §3.7.2, §8, §9, TRANSPORT_TCP §2.2)
//!
//! - Listener `addr` must parse as a valid `SocketAddr`.
//! - `kind: ws` requires `path` starting with `/`.
//! - `profile: plain` may only bind loopback (127.0.0.0/8 or ::1).
//! - `kind: ws` + `profile: tls_no_export` (browser binding_mode = 0x02)
//!   REQUIRES a non-empty `browser_origin_allowlist` (per spec §9 origin
//!   pinning).
//! - All `KdfConfig` blocks must satisfy the §3.7.2 floor:
//!   `memory_kb >= 19_456`, `time >= 2`, `parallelism >= 1`,
//!   `argon2_version == 0x13`.
//! - `argon2_concurrent_max` must be in `1..=1024`.
//
// Wave-1 deliverable. Consumed by `connection.rs` (listener spawn) and
// `main.rs` (boot path).

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Top-level configuration as loaded from the `.ktav` file.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Root directory for durable state (server_meta, user_directory,
    /// audit log, redb databases).
    pub data_dir: PathBuf,
    /// Tracing / logging configuration.
    #[serde(default)]
    pub logging: LoggingConfig,
    /// Default KDF parameters applied to every listener that does not
    /// override them.
    pub kdf_defaults: KdfConfig,
    /// Server-wide cap on concurrent Argon2id derivations (spec §8 = 64).
    #[serde(default = "default_argon2_max")]
    pub argon2_concurrent_max: u32,
    /// One entry per network endpoint the server should expose.
    pub listeners: Vec<ListenerConfig>,
    /// TLS material for the TLS-bearing listeners.
    pub tls: TlsConfig,
    /// Connection / per-request security knobs.
    #[serde(default)]
    pub security: SecurityConfig,
    /// Audit log knobs (rotation, retention).
    #[serde(default)]
    pub audit: AuditConfig,
    /// Observability HTTP server (`/healthz` etc.). Empty block = bind
    /// to default loopback `127.0.0.1:9090`. Set `addr = ""` to disable.
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

/// Observability HTTP server (`/healthz`, `/readyz`, `/metrics`, `/info`).
#[derive(Debug, Clone, Deserialize)]
pub struct ObservabilityConfig {
    /// Bind address. Default `127.0.0.1:9090`. Empty string disables
    /// the server entirely (no port bound, no metrics endpoint, no
    /// liveness probe — typically you don't want this in production).
    #[serde(default = "default_observability_addr")]
    pub addr: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            addr: default_observability_addr(),
        }
    }
}

fn default_observability_addr() -> String {
    "127.0.0.1:9090".to_string()
}

/// Audit log file management.
#[derive(Debug, Clone, Deserialize)]
pub struct AuditConfig {
    /// Max size of the active audit log file before it is
    /// rotated. `0` disables rotation. Default 100 MB.
    #[serde(default = "default_audit_max_size_mb")]
    pub max_file_size_mb: u64,
    /// Delete rotated audit files older than this. `0` disables cleanup
    /// (operator manages retention out-of-band). Default 30 days.
    #[serde(default = "default_audit_retention_days")]
    pub retention_days: u32,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            max_file_size_mb: default_audit_max_size_mb(),
            retention_days: default_audit_retention_days(),
        }
    }
}

fn default_audit_max_size_mb() -> u64 {
    100
}
fn default_audit_retention_days() -> u32 {
    30
}

/// Connection-level security limits — apply to every listener.
#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    /// Slow-loris defence: max wall-clock time to wait for the client's
    /// `auth_init` after the TLS handshake completes. Real clients send
    /// it within ~50 ms; the default 5 s is comfortably above network
    /// jitter while still cutting attackers off quickly.
    #[serde(default)]
    pub connection: ConnectionSecurity,
    /// Hard caps on per-batch resources. Applied as a max — the client's
    /// `BatchRequest.limits` may shrink them, but cannot exceed.
    #[serde(default)]
    pub query_limits: QueryLimitsConfig,
    /// Hard cap on per-interactive-tx staged bytes.
    #[serde(default)]
    pub tx: TxLimitsConfig,
    /// Per-subnet `auth_init` rate limit (token-bucket, spec §8).
    /// Each `/24` IPv4 or `/64` IPv6 subnet gets this many tokens per
    /// second. Default 10. Must be in `1..=100_000`.
    #[serde(default = "default_auth_init_rate_per_second")]
    pub auth_init_rate_per_second: u32,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            connection: Default::default(),
            query_limits: Default::default(),
            tx: Default::default(),
            auth_init_rate_per_second: default_auth_init_rate_per_second(),
        }
    }
}

/// Server-side hard cap on per-interactive-tx staged bytes.
///
/// Without this block the default 64 MiB cap applies. A malicious client
/// staging unbounded data inside a single interactive tx would grow the WAL
/// entry and in-memory staging until OOM — the cap aborts the tx with
/// `tx_too_large` before that happens.
#[derive(Debug, Clone, Deserialize)]
pub struct TxLimitsConfig {
    /// Maximum total bytes that an interactive tx may stage across all
    /// its `TxExecute` calls. Over-budget aborts the tx with
    /// `tx_too_large`. Default 64 MiB.
    #[serde(default = "default_max_tx_bytes")]
    pub max_tx_bytes: usize,
}

impl Default for TxLimitsConfig {
    fn default() -> Self {
        Self {
            max_tx_bytes: default_max_tx_bytes(),
        }
    }
}

fn default_max_tx_bytes() -> usize {
    64 * 1024 * 1024 // 64 MiB
}

/// Server-side hard caps on `BatchRequest.limits`.
///
/// Without this block, the limits the server applies come from the client
/// payload (`BatchRequest.limits`) — meaning a malicious client can ask for
/// `max_result_size = 1 TB` and the server will trust it. With it, the
/// server always clamps each field to the configured cap.
#[derive(Debug, Clone, Deserialize)]
pub struct QueryLimitsConfig {
    /// Maximum total result size (bytes) — clamps `BatchLimits::max_result_size`.
    /// Default 1 GiB.
    #[serde(default = "default_max_result_size_bytes")]
    pub max_result_size_bytes: usize,
    /// Maximum total execution time (seconds) — clamps
    /// `BatchLimits::max_execution_time_secs`. Default 60.
    #[serde(default = "default_max_execution_time_secs")]
    pub max_execution_time_secs: u64,
    /// Maximum number of queries per batch — clamps `BatchLimits::max_queries`.
    /// Default 100.
    #[serde(default = "default_max_queries_per_batch")]
    pub max_queries_per_batch: usize,
}

impl Default for QueryLimitsConfig {
    fn default() -> Self {
        Self {
            max_result_size_bytes: default_max_result_size_bytes(),
            max_execution_time_secs: default_max_execution_time_secs(),
            max_queries_per_batch: default_max_queries_per_batch(),
        }
    }
}

fn default_max_result_size_bytes() -> usize {
    1024 * 1024 * 1024 // 1 GiB
}
fn default_max_execution_time_secs() -> u64 {
    60
}
fn default_max_queries_per_batch() -> usize {
    100
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionSecurity {
    /// Slow-loris timeout for `auth_init` in milliseconds. Default 5000.
    #[serde(default = "default_auth_init_timeout_ms")]
    pub auth_init_timeout_ms: u64,
    /// Global hard cap on simultaneously-active connections across all
    /// listeners. Reached → server closes the new TCP socket immediately
    /// (TCP RST, no TLS handshake) so an attacker can't waste server CPU
    /// on TLS for connections that won't be served. Default 10000.
    #[serde(default = "default_max_active_connections")]
    pub max_active_connections: usize,
}

impl Default for ConnectionSecurity {
    fn default() -> Self {
        Self {
            auth_init_timeout_ms: default_auth_init_timeout_ms(),
            max_active_connections: default_max_active_connections(),
        }
    }
}

fn default_auth_init_timeout_ms() -> u64 {
    5_000
}

fn default_max_active_connections() -> usize {
    10_000
}

/// Logging / tracing configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    /// Log level: `trace` | `debug` | `info` | `warn` | `error`.
    /// Defaults to `info` when the whole `logging` block is omitted.
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Log a `WARN` line for every batch whose `execution_time_us`
    /// exceeds this many milliseconds. Set to `0` to disable. Default
    /// 1000 ms (1 second).
    #[serde(default = "default_slow_query_threshold_ms")]
    pub slow_query_threshold_ms: u64,
    /// Optional file path for batched log output. When `None` (default)
    /// logs go to stdout via the non-blocking appender (slice 1). When
    /// `Some(path)`, logs are written to a file through an in-memory
    /// buffer flushed every `flush_interval_ms` or on shutdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// How often (ms) the batched file writer flushes its in-memory
    /// buffer to disk. Only used when `file` is `Some`. Default 2000 ms.
    #[serde(default = "default_log_flush_interval_ms")]
    pub flush_interval_ms: u64,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            slow_query_threshold_ms: default_slow_query_threshold_ms(),
            file: None,
            flush_interval_ms: default_log_flush_interval_ms(),
        }
    }
}

fn default_slow_query_threshold_ms() -> u64 {
    1_000
}

fn default_log_flush_interval_ms() -> u64 {
    2_000
}

/// Argon2id KDF parameters (spec §3.7).
///
/// `argon2_version` is the raw protocol byte — `0x13` is the only
/// value the spec permits today.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct KdfConfig {
    /// Memory cost in KiB. Spec §3.7.2 floor = 19_456 (RFC 9106 minimum).
    pub memory_kb: u32,
    /// Iteration count. Spec §3.7.2 floor = 2.
    pub time: u32,
    /// Lanes. Spec §3.7.2 floor = 1.
    pub parallelism: u32,
    /// Argon2 protocol version byte. Must be `0x13`.
    pub argon2_version: u8,
}

/// One listener (TCP or WebSocket) the server should expose.
#[derive(Debug, Clone, Deserialize)]
pub struct ListenerConfig {
    /// Wire protocol: native TCP or WebSocket.
    pub kind: ListenerKind,
    /// Socket address, e.g. `0.0.0.0:7331` or `127.0.0.1:7334`. Validated
    /// at [`Config::validate`] time, not at deserialization.
    pub addr: String,
    /// Security profile (drives `binding_mode` per spec §3.4 / §9).
    pub profile: ProfileKind,
    /// HTTP path for `ws` listeners. Required for `ws`, ignored for `tcp`.
    #[serde(default)]
    pub path: Option<String>,
    /// Per-listener override of the default KDF parameters. Used to give
    /// browser endpoints a softer floor (cf. `docs/roadmap/BROWSER_WASM_PLAN.md`).
    #[serde(default)]
    pub kdf_override: Option<KdfConfig>,
    /// Origin header allowlist. REQUIRED for browser endpoints (`ws` +
    /// `tls_no_export`); ignored otherwise.
    #[serde(default)]
    pub browser_origin_allowlist: Vec<String>,
}

/// Listener wire protocol.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListenerKind {
    /// Native TCP (with or without TLS depending on profile).
    Tcp,
    /// WebSocket-over-TLS (`wss://`).
    Ws,
}

/// Listener security profile / `binding_mode` selector.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// TLS 1.3 with channel binding via TLS exporter (`binding_mode = 0x01`).
    /// Default profile. Allowed on any address.
    TlsExporter,
    /// TLS 1.3 without exporter (`binding_mode = 0x02`). Used for browser
    /// WebSocket endpoints where the JS environment can't access the
    /// exporter.
    TlsNoExport,
    /// Plain TCP (`binding_mode = 0x00`). Loopback-only — refused on any
    /// non-loopback address by [`Config::validate`].
    Plain,
}

/// TLS server material. Both files MUST be PEM. If either path does not
/// exist on first start, the server generates a self-signed pair and
/// writes them (handled by the boot path, not this module).
#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    /// Path to the X.509 server certificate (PEM).
    pub cert_path: PathBuf,
    /// Path to the matching private key (PEM, PKCS#8 or SEC1).
    pub key_path: PathBuf,
}

/// Configuration error covering both load failure and validation failure.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Ktav parser rejected the input. Wraps the formatted ktav error.
    #[error("config parse: {0}")]
    Parse(String),
    /// One of the [`Config::validate`] invariants was violated.
    #[error("config validation: {0}")]
    Validation(String),
    /// Underlying I/O error (file missing, permission denied, etc.).
    #[error("config io: {0}")]
    Io(#[from] std::io::Error),
}

fn default_log_level() -> String {
    "info".into()
}

fn default_argon2_max() -> u32 {
    64
}

fn default_auth_init_rate_per_second() -> u32 {
    10
}

// Spec §3.7.2 KDF floors.
const KDF_MIN_MEMORY_KB: u32 = 19_456;
const KDF_MIN_TIME: u32 = 2;
const KDF_MIN_PARALLELISM: u32 = 1;
const KDF_REQUIRED_VERSION: u8 = 0x13;

const ARGON2_MAX_PERMITS_FLOOR: u32 = 1;
const ARGON2_MAX_PERMITS_CEIL: u32 = 1024;

impl Config {
    /// Load and parse a `.ktav` config file. Does NOT validate semantics;
    /// call [`Config::validate`] separately for that.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        // ktav::from_file returns its own error type (which itself wraps
        // io::Error in some variants). We flatten it to a string so callers
        // get a single `Parse` variant; that keeps `ConfigError::Io`
        // reserved for io we surface ourselves later (e.g. dir creation).
        ktav::from_file::<Self, _>(path).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Sanity-check a parsed [`Config`]. Returns
    /// [`ConfigError::Validation`] with a human-readable message at the
    /// first violation; callers can surface that as a startup error.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // KDF: defaults first, then any per-listener override.
        validate_kdf(&self.kdf_defaults)
            .map_err(|m| ConfigError::Validation(format!("kdf_defaults: {m}")))?;

        // Argon2 concurrency cap.
        if !(ARGON2_MAX_PERMITS_FLOOR..=ARGON2_MAX_PERMITS_CEIL)
            .contains(&self.argon2_concurrent_max)
        {
            return Err(ConfigError::Validation(format!(
                "argon2_concurrent_max must be in 1..=1024 (got {})",
                self.argon2_concurrent_max
            )));
        }

        // Auth-init rate limit.
        if !(1..=100_000).contains(&self.security.auth_init_rate_per_second) {
            return Err(ConfigError::Validation(format!(
                "security.auth_init_rate_per_second must be in 1..=100_000 (got {})",
                self.security.auth_init_rate_per_second
            )));
        }

        if self.logging.flush_interval_ms == 0 {
            return Err(ConfigError::Validation(
                "logging.flush_interval_ms must be >= 1".into(),
            ));
        }

        if self.listeners.is_empty() {
            return Err(ConfigError::Validation(
                "at least one listener is required".into(),
            ));
        }

        for (idx, l) in self.listeners.iter().enumerate() {
            validate_listener(idx, l)?;
            if let Some(kdf) = &l.kdf_override {
                validate_kdf(kdf).map_err(|m| {
                    ConfigError::Validation(format!("listeners[{idx}].kdf_override: {m}"))
                })?;
            }
        }

        Ok(())
    }
}

/// Validate a single [`ListenerConfig`] against its profile's invariants.
fn validate_listener(idx: usize, l: &ListenerConfig) -> Result<(), ConfigError> {
    // 1. Parse the address.
    let addr: SocketAddr = l.addr.parse().map_err(|e| {
        ConfigError::Validation(format!(
            "listeners[{idx}].addr `{}` is not a valid socket address: {e}",
            l.addr
        ))
    })?;

    // 2. WebSocket: path required and must start with `/`.
    match l.kind {
        ListenerKind::Ws => {
            let path = l.path.as_deref().ok_or_else(|| {
                ConfigError::Validation(format!(
                    "listeners[{idx}] kind=ws requires `path` (e.g. /shamir/v1)"
                ))
            })?;
            if !path.starts_with('/') {
                return Err(ConfigError::Validation(format!(
                    "listeners[{idx}].path `{path}` must start with `/`"
                )));
            }
        }
        ListenerKind::Tcp => {
            // path is ignored for TCP — but we don't reject Some(_), so
            // operators can keep one shared template if they like.
        }
    }

    // 3. Plain profile: loopback only.
    if l.profile == ProfileKind::Plain && !is_loopback_ip(addr.ip()) {
        return Err(ConfigError::Validation(format!(
            "listeners[{idx}] profile=plain requires a loopback address \
             (127.0.0.0/8 or ::1); got {}",
            addr
        )));
    }

    // 4. Browser endpoint: must have a non-empty Origin allowlist.
    if l.kind == ListenerKind::Ws
        && l.profile == ProfileKind::TlsNoExport
        && l.browser_origin_allowlist.is_empty()
    {
        return Err(ConfigError::Validation(format!(
            "listeners[{idx}] (ws + tls_no_export, browser endpoint) \
             requires non-empty browser_origin_allowlist (spec §9)"
        )));
    }

    Ok(())
}

/// Validate a [`KdfConfig`] against the spec §3.7.2 floor. Returns the
/// raw failure reason (no listener-prefix) so callers can wrap it with
/// the right context.
fn validate_kdf(kdf: &KdfConfig) -> Result<(), String> {
    if kdf.memory_kb < KDF_MIN_MEMORY_KB {
        return Err(format!(
            "kdf memory_kb must be >= {KDF_MIN_MEMORY_KB} (got {})",
            kdf.memory_kb
        ));
    }
    if kdf.time < KDF_MIN_TIME {
        return Err(format!(
            "kdf time must be >= {KDF_MIN_TIME} (got {})",
            kdf.time
        ));
    }
    if kdf.parallelism < KDF_MIN_PARALLELISM {
        return Err(format!(
            "kdf parallelism must be >= {KDF_MIN_PARALLELISM} (got {})",
            kdf.parallelism
        ));
    }
    if kdf.argon2_version != KDF_REQUIRED_VERSION {
        return Err(format!(
            "kdf argon2_version must be 0x{KDF_REQUIRED_VERSION:02x} (got 0x{:02x})",
            kdf.argon2_version
        ));
    }
    Ok(())
}

/// Loopback predicate matching TRANSPORT_TCP §2.2 (127.0.0.0/8 + ::1).
/// Inlined rather than calling `shamir_transport_tcp::listener::validate_addr`
/// because that function takes a `ListenerProfile` enum we'd otherwise have
/// to translate from `ProfileKind`; the loopback check itself is a one-liner.
fn is_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}
