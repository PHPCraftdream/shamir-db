//! Delta file for the `conservation` gate: declared allowances for a
//! staged refactor.
//!
//! Shape (all fields required; missing/empty fields are
//! `MALFORMED_DELTA` findings):
//!
//! ```toml
//! base = "abc123..."          # REQUIRED: expected base SHA (abbreviated ok)
//!
//! [[removed]]                 # an item intentionally deleted
//! path = "crates/x/src/old.rs"   # base file the item lived in
//! `item`   = "shamir_x::old::Plan"  # full logical item path (exact)
//!                                # match — no `::name` tails, no bare
//!                                # names: loose matching could pair the
//!                                # allowance with the wrong item
//! reason = "superseded by Planner"
//! owner  = "@alice"
//!
//! [[added]]                   # a genuinely new item
//! path = "crates/x/src/planner.rs"
//! item = "Plan"
//! reason = "new planning stage"
//! owner = "@alice"
//!
//! [[changed]]                 # body fingerprint change, accepted
//! path = "crates/x/src/planner.rs"
//! item = "Planner::plan"
//! reason = "new spill heuristic"
//! owner = "@bob"
//!
//! [[moved]]                   # item relocated (file or module move)
//! path = "crates/x/src/old.rs"       # base side
//! item = "Plan"
//! to_path = "crates/x/src/planner.rs" # head side (optional)
//! to_item = "Plan"                    # head side (optional)
//! reason = "moved to planner module"
//! owner = "@alice"
//!
//! [[renamed]]                 # same everything, new name
//! path = "crates/x/src/old.rs"
//! item = "Plan"
//! to_item = "Draft"
//! reason = "clearer name"
//! owner = "@alice"
//!
//! [[extracted]]               # one base item split into several heads
//! path = "crates/x/src/old.rs"
//! item = "Plan::run"
//! to_item = "Plan::prepare"   # first head identity (informational)
//! reason = "split prepare/execute phases"
//! owner = "@carol"
//! ```
//!
//! `TEST_LOST` is never excusable by any entry. Entries that match
//! nothing become `UNUSED_ALLOWANCE` findings.

use serde::Deserialize;

/// One declared allowance.
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub path: String,
    pub item: String,
    /// Head side of a move/rename/extract; enforced when present.
    #[serde(default)]
    pub to_path: Option<String>,
    /// Head item of a move/rename/extract; enforced when present.
    #[serde(default)]
    pub to_item: Option<String>,
    pub reason: String,
    pub owner: String,
}

/// Parsed delta file.
#[derive(Debug, Deserialize, Default)]
pub struct Delta {
    /// REQUIRED whenever a delta file is given; a file without it is
    /// `MALFORMED_DELTA` (a pin that cannot drift is no pin at all).
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default, rename = "moved")]
    pub moved: Vec<Entry>,
    #[serde(default, rename = "renamed")]
    pub renamed: Vec<Entry>,
    #[serde(default, rename = "extracted")]
    pub extracted: Vec<Entry>,
    #[serde(default, rename = "removed")]
    pub removed: Vec<Entry>,
    #[serde(default, rename = "added")]
    pub added: Vec<Entry>,
    #[serde(default, rename = "changed")]
    pub changed: Vec<Entry>,
}

/// Loads and validates the delta file; `--delta` is optional (without
/// it every difference is a finding).
pub fn load(path: Option<&str>) -> Result<Delta, String> {
    let Some(path) = path else {
        return Ok(Delta::default());
    };
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let delta: Delta =
        toml::from_str(&text).map_err(|e| format!("MALFORMED_DELTA: cannot parse {path}: {e}"))?;
    if delta
        .base
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        return Err(format!(
            "MALFORMED_DELTA: {path} is missing the required `base` SHA"
        ));
    }
    let all = delta.all();
    if all.iter().any(|e| {
        e.path.trim().is_empty()
            || e.item.trim().is_empty()
            || e.reason.trim().is_empty()
            || e.owner.trim().is_empty()
    }) {
        return Err(format!(
            "MALFORMED_DELTA: {path}: every entry needs non-empty path, item, reason, owner"
        ));
    }
    Ok(delta)
}

impl Delta {
    /// Every entry, regardless of kind.
    pub fn all(&self) -> Vec<&Entry> {
        self.moved
            .iter()
            .chain(&self.renamed)
            .chain(&self.extracted)
            .chain(&self.removed)
            .chain(&self.added)
            .chain(&self.changed)
            .collect()
    }

    /// Pops (consumes) the first unused entry of `kind` whose `path` +
    /// `item` match the base side and whose `to_path`/`to_item` (when
    /// present) match the head side.
    pub fn consume_pair(
        &mut self,
        kind: &str,
        base_path: &str,
        base_item: &str,
        head_path: &str,
        head_item: &str,
    ) -> bool {
        let list: &mut Vec<Entry> = self.list_mut(kind);
        let pos = list.iter().position(|e| {
            e.path == base_path
                && item_matches(&e.item, base_item)
                && e.to_path.as_deref().is_none_or(|p| p == head_path)
                && e.to_item
                    .as_deref()
                    .is_none_or(|i| item_matches(i, head_item))
        });
        if let Some(i) = pos {
            list.remove(i);
            return true;
        }
        false
    }

    /// Consumes a single-side entry (removed / added / changed).
    pub fn consume(&mut self, kind: &str, path: &str, item: &str) -> bool {
        let list: &mut Vec<Entry> = self.list_mut(kind);
        let pos = list
            .iter()
            .position(|e| e.path == path && item_matches(&e.item, item));
        if let Some(i) = pos {
            list.remove(i);
            return true;
        }
        false
    }

    pub(crate) fn list_mut(&mut self, kind: &str) -> &mut Vec<Entry> {
        match kind {
            "moved" => &mut self.moved,
            "renamed" => &mut self.renamed,
            "extracted" => &mut self.extracted,
            "removed" => &mut self.removed,
            "added" => &mut self.added,
            _ => &mut self.changed,
        }
    }
}

/// `item` matches a logical item path EXACTLY — no `::name` tails, no
/// bare names: suffix matching could silently pair an allowance with a
/// different item that happens to share the tail.
pub fn item_matches(pattern: &str, logical: &str) -> bool {
    logical == pattern
}
