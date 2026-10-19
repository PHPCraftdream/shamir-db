//! Parsing of `cargo nextest list --message-format json` outputs
//! (JSON-lines) into test inventories keyed by (binary id, test name).

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// One parsed list: occurrence counts per (binary id, test name)
/// identity. `BTreeMap` keeps the iteration order deterministic, which
/// makes the pairing pass (and therefore the findings) reproducible.
#[derive(Default)]
pub struct TestList {
    pub counts: BTreeMap<(String, String), u32>,
}

impl TestList {
    /// Parses JSON-lines text. Only lines with `"type": "test"` count;
    /// every other line (e.g. `"binary"` description lines) is skipped.
    /// An unparseable line is an error — the caller reports it as a
    /// `BLOCKED_COMPARISON` finding instead of silently truncating the
    /// baseline (fail closed).
    pub fn parse(text: &str) -> Result<TestList, String> {
        let mut list = TestList::default();
        for (idx, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(line)
                .map_err(|e| format!("line {}: not valid JSON: {e}", idx + 1))?;
            if v.get("type").and_then(Value::as_str) != Some("test") {
                continue;
            }
            let Some(test) = v.get("test").and_then(Value::as_str) else {
                continue;
            };
            let binary = binary_id(&v)
                .ok_or_else(|| format!("line {}: test entry without a binary id", idx + 1))?;
            *list.counts.entry((binary, test.to_owned())).or_default() += 1;
        }
        Ok(list)
    }

    /// The distinct binary/target ids carrying at least one test.
    pub fn binaries(&self) -> BTreeSet<String> {
        self.counts.keys().map(|(b, _)| b.clone()).collect()
    }

    /// Identities that occur more than once in the list.
    pub fn duplicates(&self) -> impl Iterator<Item = &(String, String)> {
        self.counts
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(identity, _)| identity)
    }
}

/// Tolerant binary-id extraction: nextest emits the binary id either as
/// a plain string under `binary`, under `binary-id`, or nested inside a
/// `binary` object as its `binary-id` field — accept all three shapes.
fn binary_id(v: &Value) -> Option<String> {
    if let Some(s) = v.get("binary").and_then(Value::as_str) {
        return Some(s.to_owned());
    }
    if let Some(s) = v.get("binary-id").and_then(Value::as_str) {
        return Some(s.to_owned());
    }
    v.get("binary")
        .and_then(|b| b.get("binary-id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}
