//! Parsing of `cargo nextest list --message-format json` outputs into
//! test inventories keyed by (binary id, test name).
//!
//! nextest emits ONE JSON document (not JSON lines):
//!
//! ```json
//! {"rust-build-meta": {...}, "test-count": 2,
//!  "rust-suites": {"<binary-id>": {"binary-id": "<binary-id>",
//!    "testcases": {"<test name>": {"ignored": false,
//!      "filter-match": {"status": "matches"}}}}}}
//! ```
//!
//! Unknown fields are tolerated; a document without `rust-suites`, a
//! testcase without a boolean `ignored` or a `filter-match.status`
//! string, or text that is not one JSON document at all, is a parse
//! ERROR — the caller reports it as a `BLOCKED_COMPARISON` finding instead of
//! silently truncating the baseline (fail closed). The legacy
//! JSON-lines shape is NOT parsed: it only ever existed in this
//! parser's imagination, and accepting it would let a mis-generated
//! list pass vacuously.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// One parsed list: occurrence counts and `#[ignore]` status per
/// (binary id, test name) identity. `BTreeMap` keeps the iteration
/// order deterministic, which makes the pairing pass (and therefore
/// the findings) reproducible.
#[derive(Default)]
pub struct TestList {
    pub counts: BTreeMap<(String, String), u32>,
    /// Whether each identity is `ignored` in the list (`false` when
    /// absent from the JSON).
    pub ignored: BTreeMap<(String, String), bool>,
}

impl TestList {
    /// Parses one JSON document. Requires the top-level `rust-suites`
    /// object; its keys are the binary ids and each suite's
    /// `testcases` object maps test names to testcase entries.
    pub fn parse(text: &str) -> Result<TestList, String> {
        let doc: Value =
            serde_json::from_str(text).map_err(|e| format!("not one valid JSON document: {e}"))?;
        let suites = doc
            .get("rust-suites")
            .ok_or("JSON document has no `rust-suites` field")?
            .as_object()
            .ok_or("`rust-suites` is not a JSON object")?;
        let mut list = TestList::default();
        for (binary, suite) in suites {
            let cases = suite
                .get("testcases")
                .ok_or_else(|| format!("suite `{binary}` has no `testcases` object"))?
                .as_object()
                .ok_or_else(|| format!("suite `{binary}`: `testcases` is not a JSON object"))?;
            for (test, case) in cases {
                let case = case.as_object().ok_or_else(|| {
                    format!("suite `{binary}`: testcase `{test}` is not a JSON object")
                })?;
                let ignored = case
                    .get("ignored")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| {
                        format!("suite `{binary}`: testcase `{test}` has no boolean `ignored`")
                    })?;
                let status = case
                    .get("filter-match")
                    .and_then(|filter| filter.get("status"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        format!("suite `{binary}`: testcase `{test}` has no `filter-match.status`")
                    })?;
                // A list taken with a filter expression also names the
                // tests the filter rejected; they are not inventory.
                if status != "matches" {
                    continue;
                }
                let identity = (binary.clone(), test.clone());
                *list.counts.entry(identity.clone()).or_default() += 1;
                list.ignored.insert(identity, ignored);
            }
        }
        Ok(list)
    }

    /// The distinct binary/target ids carrying at least one test.
    pub fn binaries(&self) -> BTreeSet<String> {
        self.counts.keys().map(|(b, _)| b.clone()).collect()
    }
}
