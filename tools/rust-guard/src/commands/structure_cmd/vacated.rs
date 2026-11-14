//! Base-aware comparison for files that are NEW at head.
//!
//! A layout sweep splits big files into many new ones. `structure` only
//! pairs a head file with a base file when the whole file moved
//! (`diff::find_moved`); a split-off file is "brand-new" and would be
//! judged against an empty base, so every pre-existing `pub(crate)` item,
//! `#[allow]`, inline test module or function-local `use` it carries would
//! be reported as if it were introduced by the PR.
//!
//! The VACATED part of the base is (a) every base `.rs` file that no longer
//! exists at head, and (b) the items, `#[allow]` attributes and per-file
//! findings that LEFT a base file which still exists at head (a facade that
//! shrank because its content moved into new files). This module indexes
//! them once and answers, for a file that is new at head, whether a finding
//! is backed by an identical item that left its base file. It only ever
//! REMOVES findings that are backed that way: a genuinely new item, a
//! widened item, a copy that stays in its source file or a new attribute
//! stays a finding.

use super::diff::{allow_hash_counts, allow_hash_set, rank_map, ItemKey, VisRank};
use super::owned::OwnedImplConfig;
use super::rules;
use crate::common::exceptions::Exceptions;
use crate::common::finding::Report;
use crate::common::gitfs::GitFs;
use std::collections::{BTreeMap, BTreeSet};

/// Per-file rule codes whose findings may be excused when an identical
/// finding (same code and message) exists in the vacated base files.
/// Parse and read failures, and the ownership rule, are never excused.
const EXCUSABLE: [&str; 4] = [
    "INLINE_TEST_MOD",
    "LOCAL_USE",
    "MODRS_LOGIC",
    "TESTS_MANIFEST",
];

/// Index of what left the base files (see the module doc).
pub struct Vacated {
    base_fs: Option<GitFs>,
    /// Highest visibility rank per item key over all vacated base files.
    ranks: BTreeMap<ItemKey, VisRank>,
    /// Hashed normalised `#[allow]` texts of all vacated base files.
    allows: BTreeSet<u64>,
    /// Multiset of (code, message) of the per-file rules run over them.
    findings: BTreeMap<(String, String), usize>,
}

impl Vacated {
    /// An empty index (no base revision: nothing is excused).
    pub fn empty() -> Self {
        Self {
            base_fs: None,
            ranks: BTreeMap::new(),
            allows: BTreeSet::new(),
            findings: BTreeMap::new(),
        }
    }

    /// Indexes the base files of `base_rev` that are absent from `head_fs`
    /// and what left the `selected` files that exist in both revisions.
    pub fn build(
        head_fs: &GitFs,
        base_rev: &str,
        selected: &[String],
        exceptions: &Exceptions,
        owned_cfg: &OwnedImplConfig,
    ) -> Self {
        let base_fs = match GitFs::open(Some(base_rev), head_fs.root()) {
            Ok(fs) => fs,
            Err(e) => {
                eprintln!("rust-guard: warning: cannot open base {base_rev}: {e}");
                return Self::empty();
            }
        };
        let mut paths: Vec<String> = base_fs
            .list_tree()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.ends_with(".rs") && !head_fs.exists(p))
            .collect();
        paths.sort();
        let mut out = Self::empty();
        for path in &paths {
            let Some(src) = base_fs.read(path) else {
                continue;
            };
            let Ok(file) = syn::parse_file(&src) else {
                continue;
            };
            for (key, rank) in rank_map(&file) {
                out.add_rank(key, rank);
            }
            out.allows.extend(allow_hash_set(&file));
        }
        // Per-file rule findings of the vacated files, as a multiset.
        let mut base_report = Report::new();
        rules::check_files(&base_fs, &paths, exceptions, owned_cfg, &mut base_report);
        out.add_findings(&base_report, &BTreeMap::new());
        out.add_donors(head_fs, &base_fs, selected, exceptions, owned_cfg);
        out.base_fs = Some(base_fs);
        out
    }

    /// Indexes what left the selected files that exist in both revisions
    /// (a facade shrinks when its content moves into new files): items whose
    /// key is gone from the head version of the same file, `#[allow]`
    /// attributes with fewer occurrences at head, and per-file findings
    /// that no longer occur at head. An item that stays in its source file
    /// is a copy and is never indexed.
    fn add_donors(
        &mut self,
        head_fs: &GitFs,
        base_fs: &GitFs,
        selected: &[String],
        exceptions: &Exceptions,
        owned_cfg: &OwnedImplConfig,
    ) {
        let mut donors: Vec<String> = Vec::new();
        for path in selected {
            if !path.ends_with(".rs") {
                continue;
            }
            let (Some(base_src), Some(head_src)) = (base_fs.read(path), head_fs.read(path)) else {
                continue;
            };
            if base_src == head_src {
                continue;
            }
            let (Ok(base), Ok(head)) = (syn::parse_file(&base_src), syn::parse_file(&head_src))
            else {
                continue;
            };
            let head_ranks = rank_map(&head);
            for (key, rank) in rank_map(&base) {
                if !head_ranks.contains_key(&key) {
                    self.add_rank(key, rank);
                }
            }
            let head_allows = allow_hash_counts(&head);
            for (hash, count) in allow_hash_counts(&base) {
                if count > head_allows.get(&hash).copied().unwrap_or(0) {
                    self.allows.insert(hash);
                }
            }
            donors.push(path.clone());
        }
        let mut base_report = Report::new();
        rules::check_files(base_fs, &donors, exceptions, owned_cfg, &mut base_report);
        let mut head_report = Report::new();
        rules::check_files(head_fs, &donors, exceptions, owned_cfg, &mut head_report);
        self.add_findings(&base_report, &finding_counts(&head_report));
    }

    fn add_rank(&mut self, key: ItemKey, rank: VisRank) {
        let slot = self.ranks.entry(key).or_insert(rank);
        if rank > *slot {
            *slot = rank;
        }
    }

    /// Adds the findings of `report` to the multiset, less `still_there`
    /// (findings that also occur at head did not leave the base file).
    fn add_findings(&mut self, report: &Report, still_there: &BTreeMap<(String, String), usize>) {
        let mut left = still_there.clone();
        for f in report.findings() {
            let key = (f.code.to_owned(), f.message.clone());
            if let Some(n) = left.get_mut(&key) {
                if *n > 0 {
                    *n -= 1;
                    continue;
                }
            }
            *self.findings.entry(key).or_insert(0) += 1;
        }
    }

    /// True when an item with `key` existed in a vacated base file at a
    /// visibility rank at least `rank` (so a split-off copy is no widening).
    pub(super) fn covers_vis(&self, key: &ItemKey, rank: VisRank) -> bool {
        self.ranks.get(key).is_some_and(|base| *base >= rank)
    }

    /// Hashed `#[allow]` texts present in the vacated base files.
    pub(super) fn allow_hashes(&self) -> &BTreeSet<u64> {
        &self.allows
    }

    /// Removes from `report` every excusable per-file finding of a file that
    /// is absent from base and is matched by an identical finding in the
    /// vacated base files (each base finding excuses one head finding).
    pub fn excuse_new_file_findings(&self, files: &[String], report: &mut Report) {
        let Some(base_fs) = &self.base_fs else {
            return;
        };
        let new_files: BTreeSet<&str> = files
            .iter()
            .map(String::as_str)
            .filter(|p| !base_fs.exists(p))
            .collect();
        let mut budget = self.findings.clone();
        report.retain(|f| {
            if !new_files.contains(f.path.as_str()) || !EXCUSABLE.contains(&f.code) {
                return true;
            }
            match budget.get_mut(&(f.code.to_owned(), f.message.clone())) {
                Some(left) if *left > 0 => {
                    *left -= 1;
                    false
                }
                _ => true,
            }
        });
    }
}

/// Multiset of (code, message) over a report.
fn finding_counts(report: &Report) -> BTreeMap<(String, String), usize> {
    let mut out = BTreeMap::new();
    for f in report.findings() {
        *out.entry((f.code.to_owned(), f.message.clone()))
            .or_insert(0) += 1;
    }
    out
}
