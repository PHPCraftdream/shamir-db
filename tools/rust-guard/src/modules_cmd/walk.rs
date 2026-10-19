//! Syn traversal of module trees and the `modules` gate orchestration.
//!
//! Walks every target root, resolving `mod x;` / inline `mod x { }` and
//! literal `include!` edges, keeping cfg predicates attached (never
//! erasing them). Any tracked `.rs` file under `crates/*/` or `tools/*/`
//! that is not reachable and not excepted is a finding — the "lost test"
//! case: a dropped `mod x_tests;` still compiles but fails here.

use super::resolution::{self, Resolution};
use super::roots::{self, RootKind};
use super::scan_cfg::ScanCfg;
use crate::exceptions::Exceptions;
use crate::finding::{exit_code, Report};
use crate::gitfs::GitFs;
use proc_macro2::Span;
use proc_macro2::TokenTree;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use syn::spanned::Spanned;
use syn::{Expr, File, Item, ItemMacro, ItemMod};

const DEFAULT_EXCEPTIONS: &str = ".rust-guard/exceptions.toml";
const SCOPE_PREFIXES: [&str; 2] = ["crates/", "tools/"];

/// Directory names excluded at ANY path depth, mirroring
/// `scripts/ci/check_layout.py`'s `EXCLUDED_DIR_NAMES`. `fixtures` is the
/// relevant one here: the self-test fixture trees are materialised into
/// throwaway temp repos by the CI test harness and are never part of the
/// build, so the repo-wide scan must not flag them.
const EXCLUDED_DIR_NAMES: [&str; 1] = ["fixtures"];

/// True when any path segment (excluding the file name) is excluded.
pub fn has_excluded_dir(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop(); // the file name itself is never a directory
    parts.iter().any(|seg| EXCLUDED_DIR_NAMES.contains(seg))
}

/// Context of the file currently being walked.
struct Ctx {
    file: String,
    is_root: bool,
    logical: String,
    /// Index of the target root whose tree (transitively) brought this
    /// file in — DUPLICATE_FILE is decided WITHIN one target only.
    target: usize,
    /// Package whose target root (transitively) brought this file in;
    /// producers are matched against it.
    package: String,
    /// Directory context for non-`#[path]` `mod x;` resolution.
    mod_dir: String,
    /// Base directory for `#[path]` resolution: the declaring file's
    /// directory, or the inline module's directory context when inside
    /// `mod a { ... }` (rustc rule: `#[path]` there resolves under `a/`).
    attr_dir: String,
    /// cfg predicate (parsed) under which this file was reached, if any.
    cfg: Option<Expr>,
}

/// One reaching edge of a physical file: the owning target, the logical
/// module path and the predicate it was reached under (for
/// mutual-exclusion duplicate tests).
struct Visit {
    target: usize,
    logical: String,
    pred: Option<Expr>,
}

/// Shared state for one gate run.
struct Walker<'a> {
    fs: &'a GitFs,
    exc: &'a Exceptions,
    report: Report,
    /// Physical path -> the edges that reached it.
    visited: BTreeMap<String, Vec<Visit>>,
    /// Exceptions that were actually needed (stale check).
    used: BTreeSet<String>,
    /// Producer names that actually suppressed a finding.
    used_producers: BTreeSet<String>,
    /// Repo-level scan configuration (`.rust-guard/scan.toml`).
    scan: ScanCfg,
    /// (target, physical file) pairs already traversed (children walked).
    /// `visited` records EVERY edge for duplicate accounting, but the
    /// children of a shared file must be walked once PER TARGET
    /// (finding 6): keyed only by physical path, a helper traversed
    /// under an earlier target skipped its child edges for later
    /// targets, so a real same-target double mount of a child went
    /// undetected.
    traversed: BTreeSet<(usize, String)>,
}

/// Runs the `modules` gate; returns the process exit code.
pub fn run(rev: Option<&str>, exceptions_path: Option<&str>, repo: &Path) -> u8 {
    match execute(rev, exceptions_path, repo) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rust-guard: error: {e}");
            1
        }
    }
}

/// Discovers roots and walks the tree; returns the exit code.
fn execute(rev: Option<&str>, exceptions_path: Option<&str>, repo: &Path) -> Result<u8, String> {
    let fs = GitFs::open(rev, repo)?;
    let exc_path = exceptions_path.unwrap_or(DEFAULT_EXCEPTIONS);
    let exceptions = Exceptions::load(exc_path)?;
    // Repo-level scan configuration: `exclude_paths` prefixes are skipped
    // by roots, walk and inventory (the tool's own test fixtures on the
    // real repo are excluded this way — an EXPLICIT repo setting, never a
    // hard-coded path).
    let scan = ScanCfg::load(repo)?;
    let tracked = fs.tracked_files()?;
    let scope: Vec<String> = tracked
        .iter()
        .filter(|f| in_scope(f) && f.ends_with(".rs") && !scan.excluded(f))
        .cloned()
        .collect();
    let mut walker = Walker {
        fs: &fs,
        exc: &exceptions,
        report: Report::new(),
        visited: BTreeMap::new(),
        used: BTreeSet::new(),
        used_producers: BTreeSet::new(),
        scan,
        traversed: BTreeSet::new(),
    };

    // Fail closed on manifest problems: an unreadable/unparseable
    // manifest, or one naming undeclared `required-features`, becomes a
    // BAD_MANIFEST finding instead of silently dropping its targets. A
    // failing `cargo metadata` becomes CARGO_METADATA_FAILED and the
    // scan CONTINUES on hand-parsed manifests.
    let (roots, problems) = roots::discover_with_problems(&fs, &walker.scan)?;
    for p in problems {
        // Belt-and-braces: discovery already skips excluded manifests,
        // but a problem carrying an excluded manifest path (from any
        // code path) must never reach the report.
        if walker.scan.excluded(&p.manifest) {
            continue;
        }
        walker.report.add(&p.manifest, 1, p.code, p.message);
    }

    for (idx, root) in roots.iter().enumerate() {
        if walker.scan.excluded(&root.path) {
            continue;
        }
        if !walker.fs.exists(&root.path) {
            // Register the path so `check_unaccounted` does not report
            // the already-reported missing root a second time.
            walker.visited.entry(root.path.clone()).or_default();
            walker.report.add(
                &root.path,
                1,
                "MISSING_MOD_FILE",
                "target root missing".into(),
            );
            continue;
        }
        let ctx = Ctx {
            file: root.path.clone(),
            is_root: true,
            logical: format!("{}::{}", root.package, kind_name(root.kind)),
            target: idx,
            package: root.package.clone(),
            mod_dir: resolution::mod_dir_for(&root.path, true),
            attr_dir: resolution::dir_of(&root.path),
            cfg: None,
        };
        walker.walk_file(&ctx);
    }

    walker.check_duplicates();
    walker.check_unaccounted(&scope);
    walker.check_stale();

    walker.report.print();
    Ok(exit_code(&walker.report))
}

/// Files inside the scan scope: `crates/*` and `tools/*`, never `vendor/`.
fn in_scope(path: &str) -> bool {
    SCOPE_PREFIXES.iter().any(|p| path.starts_with(p))
        && !path.starts_with("vendor/")
        && !has_excluded_dir(path)
}

fn kind_name(kind: RootKind) -> &'static str {
    match kind {
        RootKind::Lib => "lib",
        RootKind::Bin => "bin",
        RootKind::Test => "test",
        RootKind::Bench => "bench",
        RootKind::Example => "example",
        RootKind::Build => "build",
    }
}

impl Walker<'_> {
    /// Parses one file and processes its top-level items.
    fn walk_file(&mut self, ctx: &Ctx) {
        if self.scan.excluded(&ctx.file) {
            return; // excluded prefix: out of scope, never walked
        }
        let first_visit = !self.visited.contains_key(&ctx.file);
        self.visited
            .entry(ctx.file.clone())
            .or_default()
            .push(Visit {
                target: ctx.target,
                logical: ctx.logical.clone(),
                pred: ctx.cfg.clone(),
            });
        // Children are walked once per (target, file): a file shared
        // across targets is walked under EACH target that reaches it
        // (finding 6), while a second same-target edge only records its
        // Visit. Termination: a crate's module tree is a finite tree —
        // `mod` include cycles are a compile error rustc rejects — so
        // per-target traversal of every (target, file) pair is finite.
        if !self.traversed.insert((ctx.target, ctx.file.clone())) {
            return; // already walked for this target; edge recorded above
        }
        let Some(source) = self.fs.read(&ctx.file) else {
            // Report only on the first physical visit: a re-walk under a
            // later target must not duplicate the same finding.
            if first_visit {
                self.report
                    .add(&ctx.file, 1, "MISSING_MOD_FILE", "unreadable file".into());
            }
            return;
        };
        let file: File = match syn::parse_file(&source) {
            Ok(f) => f,
            Err(e) => {
                if first_visit {
                    self.report
                        .add(&ctx.file, 1, "PARSE_FAILED", format!("{e}"));
                }
                return;
            }
        };
        for item in &file.items {
            match item {
                Item::Mod(m) => self.handle_mod(ctx, m),
                Item::Macro(mac) => self.handle_macro(ctx, mac),
                _ => {}
            }
        }
    }

    /// `mod x;` / inline `mod x { ... }`, cfg predicates carried along.
    ///
    /// Inline modules are handled FIRST: an inline module needs no file
    /// of its own and extends the module-directory context, so its
    /// children resolve under `<dir>/<name>/`. Resolving the mod (which
    /// demands `x.rs`/`x/mod.rs`) before looking at `content` was a bug:
    /// `mod nested { pub mod leaf; }` wrongly demanded `nested.rs`.
    fn handle_mod(&mut self, ctx: &Ctx, m: &ItemMod) {
        let name = m.ident.to_string();
        let line = line_of(m.mod_token.span());
        let cfg: Option<Expr> = attr_cfg(&m.attrs);
        if cfg
            .as_ref()
            .map(resolution::cfg_unsatisfiable)
            .unwrap_or(false)
        {
            self.report.add(
                &ctx.file,
                line,
                "UNSATISFIABLE_CFG",
                format!("mod `{name}` is under an unsatisfiable cfg predicate"),
            );
            return;
        }
        if let Some((_, content)) = &m.content {
            self.walk_inline(ctx, &name, content, cfg);
            return;
        }
        // File-backed: build the candidate list. cfg_attr alternatives
        // are kept ALL together (mutually exclusive alternatives are not
        // duplicate modules); statically unsatisfiable alternatives are
        // dropped before resolution. Every candidate carries the AND of
        // the inherited cfg, the module's own cfg and the alternative's
        // cfg_attr predicate (composed predicates drive both the child
        // context and the mutual-exclusion duplicate test).
        let mut candidates: Vec<(Option<Expr>, Candidate, bool)> = Vec::new();
        let alts = resolution::path_alternatives(&m.attrs);
        if alts.is_empty() {
            // Default children must compose the INHERITED cfg in too
            // (finding 5a): the bare `cfg.clone()` dropped ctx.cfg, so
            // the child edge carried no predicate and mutual-exclusion
            // tests against sibling edges wrongly saw both as
            // unconditional.
            candidates.push((
                resolution::compose_cfg(&[ctx.cfg.as_ref(), cfg.as_ref()]),
                Candidate::Default,
                false,
            ));
        } else {
            let has_unconditional = alts.iter().any(|a| a.pred.is_none());
            let mut attr_preds: Vec<&Expr> = Vec::new();
            for a in &alts {
                if let Some(p) = &a.pred {
                    if !attr_preds
                        .iter()
                        .any(|q| resolution::pred_key(q) == resolution::pred_key(p))
                    {
                        attr_preds.push(p);
                    }
                }
                candidates.push((
                    resolution::compose_cfg(&[ctx.cfg.as_ref(), cfg.as_ref(), a.pred.as_ref()]),
                    Candidate::Attr(a.path.clone()),
                    false,
                ));
            }
            if !has_unconditional {
                // `#[cfg_attr(pred, path = P)]` models BOTH branches:
                // (pred → P) above, and (not(pred) → the default x.rs /
                // x/mod.rs candidates) here — ONE fallback candidate,
                // because the default branch is reachable only when ALL
                // attr predicates are false (finding 5b: one negated
                // candidate per predicate wrongly allowed the default
                // file to be "inactive" while a single pred was false).
                // For several distinct predicates the honest model is
                // `not(all(p1, ..., pn))`.
                let negated_all: Expr = if attr_preds.len() == 1 {
                    resolution::negated(attr_preds[0])
                } else {
                    syn::parse_quote!(not(all(#(#attr_preds),*)))
                };
                candidates.push((
                    resolution::compose_cfg(&[ctx.cfg.as_ref(), cfg.as_ref(), Some(&negated_all)]),
                    Candidate::Default,
                    true,
                ));
            }
        }
        candidates.retain(|(pred, _, _)| {
            !pred
                .as_ref()
                .map(resolution::cfg_unsatisfiable)
                .unwrap_or(false)
        });
        // Resolve every candidate first (finding 5c): a MISSING explicit
        // attr alternative is NOT a finding when any OTHER candidate of
        // the same `mod` resolved (the fallback or another branch) — the
        // missing branch is simply inactive under its cfg.
        let mut any_resolved = false;
        let mut missing: Vec<(bool, Vec<String>)> = Vec::new();
        for (pred, cand, fallback) in candidates {
            let res = match &cand {
                Candidate::Default => {
                    resolution::resolve(self.fs, &name, &ctx.mod_dir, &ctx.attr_dir, None)
                }
                Candidate::Attr(path) => resolution::resolve_attr(self.fs, &ctx.attr_dir, path),
            };
            match res {
                Resolution::Found(child) => {
                    any_resolved = true;
                    let mod_dir = resolution::mod_dir_for(&child, false);
                    let attr_dir = resolution::dir_of(&child);
                    let child_ctx = Ctx {
                        file: child,
                        is_root: false,
                        logical: format!("{}::{name}", ctx.logical),
                        target: ctx.target,
                        package: ctx.package.clone(),
                        mod_dir,
                        attr_dir,
                        cfg: pred,
                    };
                    self.walk_file(&child_ctx);
                }
                Resolution::Missing { tried } => missing.push((fallback, tried)),
                Resolution::Escapes(attr) => self.report.add(
                    &ctx.file,
                    line,
                    "PATH_ESCAPES_REPO",
                    format!("mod `{name}` path `{attr}` resolves outside the repository root"),
                ),
                Resolution::Ambiguous { a, b } => self.report.add(
                    &ctx.file,
                    line,
                    "AMBIGUOUS_MOD",
                    format!("mod `{name}` resolves ambiguously: both `{a}` and `{b}` exist"),
                ),
                Resolution::Generated(attr) => self.report.add(
                    &ctx.file,
                    line,
                    "UNRESOLVED_INCLUDE",
                    format!("mod `{name}` path `{attr}` is generated; declare a [[producer]]"),
                ),
            }
        }
        if !any_resolved {
            // ONE finding per `mod`: previously a MISSING_MOD_FILE was
            // emitted per missing non-fallback candidate, duplicating the
            // same mod/line once per attr alternative. Aggregate all
            // tried-path lists into a single report. The fallback branch
            // contributes its paths too (when NOTHING resolves — only
            // then do we report — the fallback's absence is part of the
            // same failure, so one finding covers it, not zero-plus-one).
            let mut all_tried: Vec<String> = Vec::new();
            for (fallback, tried) in missing {
                if fallback && !all_tried.is_empty() {
                    // The cfg_attr fallback branch resolving to nothing is
                    // expected when an attr twin already reported; skip if
                    // any non-fallback candidate was missing.
                    continue;
                }
                for t in tried {
                    if !all_tried.contains(&t) {
                        all_tried.push(t);
                    }
                }
            }
            if !all_tried.is_empty() {
                self.report.add(
                    &ctx.file,
                    line,
                    "MISSING_MOD_FILE",
                    format!(
                        "mod `{name}` file not found (tried {})",
                        all_tried.join(", ")
                    ),
                );
            }
        }
    }

    /// Inline `mod x { ... }`: same physical file, logical path extended,
    /// and the module name APPENDED to both resolution contexts
    /// (edition-2018+ rule: `mod a { mod b; }` in `src/lib.rs` resolves
    /// `b` at `src/a/b.rs`; inline-in-inline composes: `a/b/c.rs`).
    fn walk_inline(&mut self, ctx: &Ctx, name: &str, content: &Vec<Item>, own_cfg: Option<Expr>) {
        let mod_dir = if ctx.mod_dir.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{}", ctx.mod_dir, name)
        };
        let inner = Ctx {
            file: ctx.file.clone(),
            is_root: ctx.is_root,
            logical: format!("{}::{name}", ctx.logical),
            target: ctx.target,
            package: ctx.package.clone(),
            mod_dir: mod_dir.clone(),
            attr_dir: mod_dir,
            // The inline module's OWN cfg AND-s into the inherited one:
            // everything declared inside is guarded by both.
            cfg: resolution::compose_cfg(&[ctx.cfg.as_ref(), own_cfg.as_ref()]),
        };
        for item in content {
            match item {
                Item::Mod(m) => self.handle_mod(&inner, m),
                Item::Macro(mac) => self.handle_macro(&inner, mac),
                _ => {}
            }
        }
    }

    /// Module-level macros: literal `include!` edges and candidate
    /// module-producing macros.
    fn handle_macro(&mut self, ctx: &Ctx, mac: &ItemMacro) {
        let name = mac
            .mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        let line = line_of(mac.span());
        if name == "include" {
            self.handle_include(ctx, mac, line);
            return;
        }
        // Any other module-level macro whose tokens mention `mod` may
        // produce modules the parser cannot see; require a producer.
        if mentions_mod(&mac.mac.tokens) {
            if self.exc.has_producer(&name, &ctx.package) {
                self.used_producers.insert(name);
            } else {
                self.report.add(
                    &ctx.file,
                    line,
                    "UNRESOLVED_MACRO",
                    format!("macro `{name}!` may declare modules; declare it in [[producer]]"),
                );
            }
        }
    }

    /// `include!("literal")` of a tracked `.rs` file is a source edge. A
    /// NON-literal include (`concat!(env!("OUT_DIR"), ...)`, build-time
    /// paths) cannot be resolved statically — fail closed: finding unless
    /// an `include` [[producer]] covers this package (code chosen over
    /// UNRESOLVED_MACRO: the unresolved thing is an include edge).
    fn handle_include(&mut self, ctx: &Ctx, mac: &ItemMacro, line: usize) {
        // Only a macro invocation whose ENTIRE token stream is one string
        // literal is a static include edge. A non-literal form
        // (`concat!(env!("OUT_DIR"), "/g.rs")`, any computed path) would
        // otherwise have its inner `"/g.rs"` literal mistaken for the
        // whole path and "resolved" against the wrong location.
        let lit = match mac.mac.tokens.clone().into_iter().next() {
            Some(TokenTree::Literal(l)) if l.to_string().starts_with('"') => {
                syn::parse_str::<syn::LitStr>(&l.to_string())
                    .ok()
                    .map(|ls| ls.value())
            }
            _ => None,
        };
        let Some(lit) = lit else {
            if self.exc.has_producer("include", &ctx.package) {
                self.used_producers.insert("include".to_owned());
            } else {
                self.report.add(
                    &ctx.file,
                    line,
                    "UNRESOLVED_INCLUDE",
                    "non-literal include! cannot be resolved; declare an `include` [[producer]]"
                        .into(),
                );
            }
            return;
        };
        match resolution::resolve_include(self.fs, &ctx.file, &lit) {
            Resolution::Found(target) => {
                if target.ends_with(".rs") {
                    let mod_dir = resolution::mod_dir_for(&target, false);
                    let attr_dir = resolution::dir_of(&target);
                    let logical = format!("{}::include({lit})", ctx.logical);
                    let child = Ctx {
                        file: target,
                        is_root: false,
                        logical,
                        target: ctx.target,
                        package: ctx.package.clone(),
                        mod_dir,
                        attr_dir,
                        cfg: ctx.cfg.clone(),
                    };
                    self.walk_file(&child);
                }
            }
            Resolution::Generated(raw) => {
                if self.exc.has_producer("include", &ctx.package) {
                    self.used_producers.insert("include".to_owned());
                } else {
                    self.report.add(
                        &ctx.file,
                        line,
                        "UNRESOLVED_INCLUDE",
                        format!("generated include `{raw}`; declare an `include` [[producer]]"),
                    );
                }
            }
            Resolution::Escapes(raw) => self.report.add(
                &ctx.file,
                line,
                "PATH_ESCAPES_REPO",
                format!("include path `{raw}` resolves outside the repository root"),
            ),
            Resolution::Missing { tried } => self.report.add(
                &ctx.file,
                line,
                "UNRESOLVED_INCLUDE",
                format!("include target not found (tried {})", tried.join(", ")),
            ),
            Resolution::Ambiguous { .. } => {}
        }
    }

    /// A physical file reached by more than one logical path WITHIN ONE
    /// target's module tree is a duplicate — unless an exception covers
    /// it, or every differing pair of reaching edges is provably mutually
    /// exclusive (alternative cfg_attr branches such as `unix` vs
    /// `not(unix)`, compared on the COMPOSED predicates).
    ///
    /// Reaching a file from SEVERAL targets (a shared helper bench root
    /// also `mod`-included elsewhere, `tests/harness.rs` auto-discovered
    /// AND `mod harness;` from each test binary) is normal cargo
    /// behaviour and never a duplicate — finding 1 of the real-repo CI
    /// run, where shared bench helpers were flagged four times.
    fn check_duplicates(&mut self) {
        let mut findings: Vec<(String, usize, String)> = Vec::new();
        for (path, visits) in &self.visited {
            // Distinct logical entries per target.
            let mut per_target: BTreeMap<usize, Vec<(&String, Option<&Expr>)>> = BTreeMap::new();
            for v in visits {
                let entries = per_target.entry(v.target).or_default();
                if !entries.iter().any(|(l, _)| *l == &v.logical) {
                    entries.push((&v.logical, v.pred.as_ref()));
                }
            }
            // An exception is consumed ONLY when there is an actual
            // duplicate to suppress (finding 10): marking it used for a
            // merely-visited path hid stale exceptions for files that
            // are reachable exactly once (no conflict at all).
            let mut duplicate = false;
            let mut all_paths: Vec<&str> = Vec::new();
            for entries in per_target.values() {
                if entries.len() <= 1 {
                    continue;
                }
                for (l, _) in entries {
                    all_paths.push(l);
                }
                for i in 0..entries.len() {
                    for j in (i + 1)..entries.len() {
                        let exclusive = match (entries[i].1, entries[j].1) {
                            (Some(a), Some(b)) => resolution::mutually_exclusive(a, b),
                            _ => false,
                        };
                        if !exclusive {
                            duplicate = true;
                        }
                    }
                }
            }
            if duplicate {
                if self.exc.by_path(path).is_some() {
                    self.used.insert(path.clone());
                    continue;
                }
                all_paths.sort_unstable();
                findings.push((
                    path.clone(),
                    1,
                    format!(
                        "mounted under conflicting logical paths in one target: {}",
                        all_paths.join(" | ")
                    ),
                ));
            }
        }
        for (path, line, msg) in findings {
            self.report.add(&path, line, "DUPLICATE_FILE", msg);
        }
    }

    /// Any tracked, in-scope `.rs` file that was never reached must be
    /// excepted — this is the lost-test detection.
    fn check_unaccounted(&mut self, scope: &[String]) {
        let mut findings: Vec<(String, String)> = Vec::new();
        for path in scope {
            if self.visited.contains_key(path) {
                continue;
            }
            if self.exc.by_path(path).is_some() {
                self.used.insert(path.clone());
            } else {
                findings.push((
                    path.clone(),
                    "tracked Rust file reachable from no target root and not excepted".into(),
                ));
            }
        }
        for (path, msg) in findings {
            self.report.add(&path, 1, "UNACCOUNTED_FILE", msg);
        }
    }

    /// Exceptions/producers whose entry is no longer needed fail the run.
    fn check_stale(&mut self) {
        let mut stale: Vec<(String, String)> = self
            .exc
            .exceptions
            .iter()
            .filter(|e| !self.used.contains(&e.path))
            .map(|e| {
                (
                    e.path.clone(),
                    format!(
                        "exception for package {} (owner {}) no longer needed: {}",
                        e.package, e.owner, e.reason
                    ),
                )
            })
            .collect();
        for p in &self.exc.producers {
            if !self.used_producers.contains(&p.name) {
                stale.push((
                    format!("[[producer]] {}", p.name),
                    format!(
                        "producer (owner {}) never suppressed a finding: {}",
                        p.owner, p.reason
                    ),
                ));
            }
        }
        for (path, msg) in stale {
            self.report.add(&path, 1, "STALE_EXCEPTION", msg);
        }
    }
}

// ---------- attribute / token helpers ----------

/// Which resolution strategy a candidate uses.
enum Candidate {
    /// Default `x.rs` / `x/mod.rs` rules.
    Default,
    /// An explicit (possibly cfg_attr-gated) path attribute.
    Attr(String),
}

/// Extracts a parsed `#[cfg(...)]` predicate, if present.
fn attr_cfg(attrs: &[syn::Attribute]) -> Option<Expr> {
    attrs
        .iter()
        .find(|a| a.path().is_ident("cfg"))
        .and_then(|a| a.parse_args::<Expr>().ok())
}

/// True when the token stream mentions the `mod` keyword anywhere —
/// token GROUPS are descended into, so a macro body like
/// `{ mod generated; }` (nested braces of `macro_rules!`) is detected;
/// a top-level-only scan misses exactly the real macro producers.
fn mentions_mod(tokens: &proc_macro2::TokenStream) -> bool {
    tokens.clone().into_iter().any(|t| match t {
        TokenTree::Ident(id) => id == "mod",
        TokenTree::Group(g) => mentions_mod(&g.stream()),
        _ => false,
    })
}

/// Line number of a span (needs proc-macro2 `span-locations`).
fn line_of(span: Span) -> usize {
    span.start().line
}
