use serde::Deserialize;

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

fn default_log_level() -> String {
    "info".into()
}

fn default_slow_query_threshold_ms() -> u64 {
    1_000
}

fn default_log_flush_interval_ms() -> u64 {
    2_000
}
