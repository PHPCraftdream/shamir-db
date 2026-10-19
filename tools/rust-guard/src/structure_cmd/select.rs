//! File selection, glob matching, base-revision resolution and the
//! delta file for the `structure` gate.

use crate::finding::{exit_code, Report};
use crate::gitfs::GitFs;
use serde::Deserialize;
use syn::Item;

use super::{diff, owned, rules};

/// CLI arguments for `rust-guard structure` (declared in `main.rs`).
pub struct StructureArgs {
    /// Repository root (the `--repo` flag).
    pub repo: std::path::PathBuf,
    pub paths: Vec<String>,
    pub changed_since: Option<String>,
    pub base: Option<String>,
    pub delta: Option<String>,
    pub exceptions: Option<String>,
    pub warn_primary_export: bool,
}

/// Delta-file record: an accepted visibility widening or new `#[allow]`.
/// Matching is EXACT on (path, item); `reason` and `owner` document it.
/// `item` is REQUIRED — an entry without one fails to parse (fail
/// closed), there is no path-only wildcard form.
#[derive(Debug, Deserialize)]
pub struct DeltaEntry {
    pub path: String,
    pub item: String,
    pub reason: String,
    pub owner: String,
}

/// Parsed `--delta` file (both record kinds).
#[derive(Debug, Deserialize, Default)]
pub struct Delta {
    #[serde(default, rename = "allow_widen")]
    pub allow_widen: Vec<DeltaEntry>,
    #[serde(default, rename = "allow")]
    pub allow: Vec<DeltaEntry>,
}

impl Delta {
    /// Loads the delta file; a missing file is an empty set.
    pub fn load(path: Option<&str>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let delta: Delta =
            toml::from_str(&text).map_err(|e| format!("cannot parse {path}: {e}"))?;
        for e in delta.allow_widen.iter().chain(delta.allow.iter()) {
            if e.reason.trim().is_empty() || e.owner.trim().is_empty() {
                return Err(format!(
                    "{path}: delta entry for {} needs reason and owner",
                    e.path
                ));
            }
        }
        Ok(delta)
    }

    /// Consumes the delta entry (if any) covering `(code, path, item)`
    /// and reports whether one matched. Matching is EXACT: an entry
    /// matches when its `path` equals `path` AND its `item` equals
    /// `item` — there is NO wildcard form (an entry without an `item`
    /// field fails to parse outright). Each entry is consumed exactly
    /// once: a second identical finding is NOT covered. Leftover entries
    /// are reported as `UNUSED_ALLOWANCE` at the end of the run, so
    /// stale entries cannot accumulate — including runs with no base
    /// revision or no selected files, where no finding could ever have
    /// consumed them.
    ///
    /// Item keys (documented contract):
    /// * `VIS_WIDENING` — the qualified item name exactly as printed in
    ///   the finding message (e.g. `m::helper`, `S::a`, `S::0` for tuple
    ///   field 0, `m::S::f` for an impl method inside inline module `m`).
    /// * `ALLOW_ADDED` — the normalised attribute text, e.g.
    ///   `#[allow(dead_code)]` (see `diff::normalise_attr`).
    pub fn consume(&mut self, code: &str, path: &str, item: &str) -> bool {
        let list = match code {
            "VIS_WIDENING" => &mut self.allow_widen,
            _ => &mut self.allow,
        };
        let pos = list.iter().position(|e| e.path == path && e.item == item);
        match pos {
            Some(i) => {
                list.remove(i);
                true
            }
            None => false,
        }
    }
}

/// Runs the `structure` gate; returns the process exit code.
pub fn run(args: StructureArgs) -> u8 {
    match execute(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rust-guard: error: {e}");
            1
        }
    }
}

fn execute(args: StructureArgs) -> Result<u8, String> {
    if args.paths.is_empty() && args.changed_since.is_none() {
        return Err("structure: at least one of --paths / --changed-since is required".into());
    }
    let fs = GitFs::open(None, &args.repo)?;
    // The delta is ALWAYS loaded, even when nothing is selected: a
    // non-empty delta in a run that can never consume its entries must
    // surface as UNUSED_ALLOWANCE, not pass silently (finding 9).
    let mut delta = Delta::load(args.delta.as_deref())?;
    let mut selected = select_files(&fs, &args)?;
    selected.sort();
    selected.dedup();
    if selected.is_empty() {
        eprintln!("rust-guard structure: no matching files");
        // No file was scanned, so no entry could have been consumed:
        // every delta entry is a leftover.
        let mut report = Report::new();
        diff::report_unused(&delta, &mut report);
        report.print();
        return Ok(exit_code(&report));
    }

    // Base revision: explicit --base, else the merge base of
    // --changed-since. Missing either skips the diff rules.
    let base = resolve_base(&fs, &args)?;
    let exceptions = crate::exceptions::Exceptions::load(
        args.exceptions
            .as_deref()
            .unwrap_or(".rust-guard/exceptions.toml"),
    )?;
    let owned = owned::OwnedImplConfig::load(owned::OWNED_IMPL_FILE)?;

    let mut report = Report::new();
    rules::check_files(&fs, &selected, &exceptions, &owned, &mut report);
    if let Some(base_rev) = &base {
        diff::check_files(&fs, base_rev, &selected, &mut delta, &mut report);
    } else {
        // No base: the diff (the only consumer of delta entries) never
        // ran, so every entry is a leftover (finding 9).
        diff::report_unused(&delta, &mut report);
    }
    if args.warn_primary_export {
        warn_primary_export(&fs, &selected);
    }
    report.print();
    Ok(exit_code(&report))
}

/// Selected repo-relative `.rs` files from `--paths` globs and/or
/// `--changed-since <rev>` (files changed since that revision).
fn select_files(fs: &GitFs, args: &StructureArgs) -> Result<Vec<String>, String> {
    let tracked = fs.tracked_files()?;
    let mut out: Vec<String> = Vec::new();
    for f in &tracked {
        if !f.ends_with(".rs") {
            continue;
        }
        if args.paths.iter().any(|g| glob_match(g, f)) {
            out.push(f.clone());
        }
    }
    if let Some(rev) = &args.changed_since {
        let mb = merge_base(fs, rev)?;
        let out_raw = git(fs, &["diff", "--name-only", &format!("{mb}..HEAD")])?;
        for line in out_raw.lines() {
            let f = line.trim();
            if f.ends_with(".rs") && !out.iter().any(|x| x == f) {
                out.push(f.to_owned());
            }
        }
    }
    Ok(out)
}

/// Base revision for the widening rules: `--base`, else the merge base
/// of `--changed-since`; `None` when neither applies.
fn resolve_base(fs: &GitFs, args: &StructureArgs) -> Result<Option<String>, String> {
    if let Some(b) = &args.base {
        return Ok(Some(b.clone()));
    }
    match &args.changed_since {
        Some(rev) => Ok(Some(merge_base(fs, rev)?)),
        None => Ok(None),
    }
}

/// Merge base of `rev` and `HEAD` (falls back to `rev` itself).
fn merge_base(fs: &GitFs, rev: &str) -> Result<String, String> {
    let out = git(fs, &["merge-base", rev, "HEAD"])?;
    let mb = out.trim().to_owned();
    if mb.is_empty() {
        Ok(rev.to_owned())
    } else {
        Ok(mb)
    }
}

/// Runs `git` in the repo root and returns stdout.
fn git(fs: &GitFs, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(fs.root())
        .output()
        .map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Minimal glob matcher over `/`-separated paths: `?` one char, `*` any
/// run within a path segment, `**` any run including `/` separators.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = path.chars().collect();
    glob_rec(&pat, &txt)
}

fn glob_rec(pat: &[char], txt: &[char]) -> bool {
    match (pat.first(), txt.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            let rest = &pat[1..];
            if rest.first() == Some(&'*') {
                let rest = rest.strip_prefix(&['*']).unwrap_or(rest);
                let rest = if rest.first() == Some(&'/') {
                    &rest[1..]
                } else {
                    rest
                };
                for i in 0..=txt.len() {
                    if glob_rec(rest, &txt[i..]) {
                        return true;
                    }
                }
                false
            } else {
                for i in 0..=txt.len() {
                    if txt[..i].contains(&'/') {
                        break;
                    }
                    if glob_rec(rest, &txt[i..]) {
                        return true;
                    }
                }
                false
            }
        }
        (Some('?'), Some(&c)) if c != '/' => glob_rec(&pat[1..], &txt[1..]),
        (Some(&p), Some(&t)) if p == t => glob_rec(&pat[1..], &txt[1..]),
        _ => false,
    }
}

/// Prints the opt-in "one primary export" WARNING (never a finding).
fn warn_primary_export(fs: &GitFs, files: &[String]) {
    for path in files {
        if path.ends_with("/mod.rs") || path == "mod.rs" {
            continue;
        }
        let Some(src) = fs.read(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else {
            continue;
        };
        let count = file
            .items
            .iter()
            .filter(|i| {
                matches!(i, Item::Fn(f) if f.vis != syn::Visibility::Inherited)
                    || matches!(i, Item::Struct(s) if s.vis != syn::Visibility::Inherited)
                    || matches!(i, Item::Enum(e) if e.vis != syn::Visibility::Inherited)
                    || matches!(i, Item::Union(u) if u.vis != syn::Visibility::Inherited)
                    || matches!(i, Item::Trait(t) if t.vis != syn::Visibility::Inherited)
            })
            .count();
        if count > 1 {
            eprintln!(
                "WARNING: {path}: declares {count} public exports (opt-in one-primary-export check)"
            );
        }
    }
}
