//! Fixed-format log line encoding for the durable audit appender.

use super::AuditEntry;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

/// Encode a free-text field for embedding in the tab-separated log line.
fn escape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str(r"\\"),
            '\t' => out.push_str(r"\t"),
            '\n' => out.push_str(r"\n"),
            c => out.push(c),
        }
    }
    out
}

/// Serialise one [`AuditEntry`] to the fixed-format log line (without
/// trailing `\n`).
pub(super) fn entry_to_line(entry: &AuditEntry) -> Vec<u8> {
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        entry.seq,
        entry.ts_ns,
        escape_field(&entry.event),
        escape_field(&entry.transport),
        escape_field(&entry.user),
        escape_field(&entry.ip_subnet),
        URL_SAFE_NO_PAD.encode(entry.session_id_prefix),
        escape_field(&entry.result),
        URL_SAFE_NO_PAD.encode(&entry.details_canonical_msgpack),
        URL_SAFE_NO_PAD.encode(entry.prev_hmac),
        URL_SAFE_NO_PAD.encode(entry.hmac),
    );
    line.into_bytes()
}
