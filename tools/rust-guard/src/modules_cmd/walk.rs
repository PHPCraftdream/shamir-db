//! Syn traversal of module trees and the `modules` gate orchestration.
//!
//! Walks every target root, resolving `mod x;` / inline `mod x { }` and
//! literal `include!` edges, keeping cfg predicates attached (never
//! erasing them). Any tracked `.rs` file under `crates/*/` or `tools/*/`
//! that is not reachable and not excepted is a finding — the "lost test"
//! case: a dropped `mod x_tests;` still compiles but fails here.

use super::resolution::{self, Resolution};
use super::roots::{self, RootKind};
use crate::exceptions::Exceptions;
use crate::finding::{exit_code, Report};
use crate::gitfs::GitFs;
use proc_macro2::Span;
use proc_macro2::TokenTree;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use syn::parse::Parse;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::Token;
use syn::{Expr, File, Item, ItemMacro, ItemMod, Lit, Meta};

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

/// One reaching edge of a physical file: the logical module path and the
/// predicate it was reached under (for mutual-exclusion duplicate tests).
struct Visit {
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
    let tracked = fs.tracked_files()?;
    let scope: Vec<String> = tracked
        .iter()
        .filter(|f| in_scope(f) && f.ends_with(".rs"))
        .cloned()
        .collect();
    let mut walker = Walker {
        fs: &fs,
        exc: &exceptions,
        report: Report::new(),
        visited: BTreeMap::new(),
        used: BTreeSet::new(),
        used_producers: BTreeSet::new(),
    };

    // Fail closed on manifest problems: an unreadable/unparseable
    // manifest, or one naming undeclared `required-features`, becomes a
    // BAD_MANIFEST finding instead of silently dropping its targets.
    let (roots, problems) = roots::discover_with_problems(&fs)?;
    for p in problems {
        walker.report.add(&p.manifest, 1, "BAD_MANIFEST", p.message);
    }

    for root in &roots {
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
        let first_visit = !self.visited.contains_key(&ctx.file);
        self.visited
            .entry(ctx.file.clone())
            .or_default()
            .push(Visit {
                logical: ctx.logical.clone(),
                pred: ctx.cfg.clone(),
            });
        if !first_visit {
            return; // duplicate edge; check_duplicates reports if undesired
        }
        let Some(source) = self.fs.read(&ctx.file) else {
            self.report
                .add(&ctx.file, 1, "MISSING_MOD_FILE", "unreadable file".into());
            return;
        };
        let file: File = match syn::parse_file(&source) {
            Ok(f) => f,
            Err(e) => {
                self.report
                    .add(&ctx.file, 1, "PARSE_FAILED", format!("{e}"));
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
            self.walk_inline(ctx, &name, content);
            return;
        }
        // File-backed: build the candidate list. cfg_attr alternatives
        // are kept ALL together (mutually exclusive alternatives are not
        // duplicate modules); statically unsatisfiable alternatives are
        // dropped before resolution.
        let mut candidates: Vec<(Option<Expr>, Candidate)> = Vec::new();
        let alts = path_alternatives(&m.attrs);
        if alts.is_empty() {
            candidates.push((None, Candidate::Default));
        } else {
            candidates.extend(alts.into_iter().map(|a| (a.pred, Candidate::Attr(a.path))));
        }
        candidates.retain(|(pred, _)| {
            !pred
                .as_ref()
                .map(resolution::cfg_unsatisfiable)
                .unwrap_or(false)
        });
        for (pred, cand) in candidates {
            // Inherited predicates compose down the tree.
            let eff = pred.or_else(|| ctx.cfg.clone());
            let res = match &cand {
                Candidate::Default => {
                    resolution::resolve(self.fs, &name, &ctx.mod_dir, &ctx.attr_dir, None)
                }
                Candidate::Attr(path) => resolution::resolve_attr(self.fs, &ctx.attr_dir, path),
            };
            match res {
                Resolution::Found(child) => {
                    let mod_dir = resolution::mod_dir_for(&child, false);
                    let attr_dir = resolution::dir_of(&child);
                    let child_ctx = Ctx {
                        file: child,
                        is_root: false,
                        logical: format!("{}::{name}", ctx.logical),
                        package: ctx.package.clone(),
                        mod_dir,
                        attr_dir,
                        cfg: eff,
                    };
                    self.walk_file(&child_ctx);
                }
                Resolution::Ambiguous { a, b } => self.report.add(
                    &ctx.file,
                    line,
                    "AMBIGUOUS_MOD",
                    format!("mod `{name}` resolves ambiguously: both `{a}` and `{b}` exist"),
                ),
                Resolution::Missing { tried } => self.report.add(
                    &ctx.file,
                    line,
                    "MISSING_MOD_FILE",
                    format!("mod `{name}` file not found (tried {})", tried.join(", ")),
                ),
                Resolution::Generated(attr) => self.report.add(
                    &ctx.file,
                    line,
                    "UNRESOLVED_INCLUDE",
                    format!("mod `{name}` path `{attr}` is generated; declare a [[producer]]"),
                ),
            }
        }
    }

    /// Inline `mod x { ... }`: same physical file, logical path extended,
    /// and the module name APPENDED to both resolution contexts
    /// (edition-2018+ rule: `mod a { mod b; }` in `src/lib.rs` resolves
    /// `b` at `src/a/b.rs`; inline-in-inline composes: `a/b/c.rs`).
    fn walk_inline(&mut self, ctx: &Ctx, name: &str, content: &Vec<Item>) {
        let mod_dir = if ctx.mod_dir.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{}", ctx.mod_dir, name)
        };
        let inner = Ctx {
            file: ctx.file.clone(),
            is_root: ctx.is_root,
            logical: format!("{}::{name}", ctx.logical),
            package: ctx.package.clone(),
            mod_dir: mod_dir.clone(),
            attr_dir: mod_dir,
            cfg: ctx.cfg.clone(),
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
        let Some(lit) = first_string_literal(&mac.mac.tokens) else {
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
            Resolution::Missing { tried } => self.report.add(
                &ctx.file,
                line,
                "UNRESOLVED_INCLUDE",
                format!("include target not found (tried {})", tried.join(", ")),
            ),
            Resolution::Ambiguous { .. } => {}
        }
    }

    /// A physical file reached by more than one logical path is a
    /// duplicate unless an exception covers it — or every differing pair
    /// of reaching edges is provably mutually exclusive (alternative
    /// cfg_attr branches: `feature = "a"` vs `feature = "b"`).
    fn check_duplicates(&mut self) {
        let mut findings: Vec<(String, usize, String)> = Vec::new();
        for (path, visits) in &self.visited {
            let mut entries: Vec<(&String, Option<&Expr>)> = visits
                .iter()
                .map(|v| (&v.logical, v.pred.as_ref()))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            entries.dedup_by(|a, b| a.0 == b.0);
            if entries.len() <= 1 {
                continue;
            }
            if self.exc.by_path(path).is_some() {
                self.used.insert(path.clone());
                continue;
            }
            let mut duplicate = false;
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
            if duplicate {
                findings.push((
                    path.clone(),
                    1,
                    format!(
                        "reached by {} logical paths: {}",
                        entries.len(),
                        entries
                            .iter()
                            .map(|(l, _)| l.as_str())
                            .collect::<Vec<_>>()
                            .join(" | ")
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

/// Extracts a parsed `#[cfg(...)]` predicate, if present.
fn attr_cfg(attrs: &[syn::Attribute]) -> Option<Expr> {
    attrs
        .iter()
        .find(|a| a.path().is_ident("cfg"))
        .and_then(|a| a.parse_args::<Expr>().ok())
}

/// One candidate file path for a `mod`, tagged with the predicate that
/// guards it (`None` for an unconditional `#[path]`).
struct PathAlt {
    pred: Option<Expr>,
    path: String,
}

/// Which resolution strategy a candidate uses.
enum Candidate {
    /// Default `x.rs` / `x/mod.rs` rules.
    Default,
    /// An explicit (possibly cfg_attr-gated) path attribute.
    Attr(String),
}

/// Collects every candidate path of a `mod` from its attributes:
/// literal `#[path = "..."]` (a NAME-VALUE attribute, read via
/// `require_name_value`) and ALL `#[cfg_attr(pred, path = "...")]`
/// alternatives — mutually exclusive branches must all stay reachable,
/// dropping all but the first wrongly hides whole modules.
fn path_alternatives(attrs: &[syn::Attribute]) -> Vec<PathAlt> {
    let mut out = Vec::new();
    for a in attrs {
        if a.path().is_ident("path") {
            if let Ok(nv) = a.meta.require_name_value() {
                if let Some(s) = lit_str_of(&nv.value) {
                    out.push(PathAlt {
                        pred: None,
                        path: s,
                    });
                }
            }
        } else if a.path().is_ident("cfg_attr") {
            if let Ok(args) = a.parse_args::<CfgAttrArgs>() {
                for meta in args.attrs {
                    if let Meta::NameValue(nv) = meta {
                        if nv.path.is_ident("path") {
                            if let Some(s) = lit_str_of(&nv.value) {
                                out.push(PathAlt {
                                    pred: Some(args.pred.clone()),
                                    path: s,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// String value of a literal expression (`"a.rs"`), if it is one.
fn lit_str_of(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        _ => None,
    }
}

/// Parsed contents of `#[cfg_attr(pred, attr, attr, ...)]`.
struct CfgAttrArgs {
    pred: Expr,
    attrs: Punctuated<Meta, Token![,]>,
}

impl Parse for CfgAttrArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let pred: Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let attrs = Punctuated::<Meta, Token![,]>::parse_terminated(input)?;
        Ok(Self { pred, attrs })
    }
}

/// First string literal in a token stream, unquoted.
fn first_string_literal(tokens: &proc_macro2::TokenStream) -> Option<String> {
    tokens.clone().into_iter().find_map(|t| match t {
        proc_macro2::TokenTree::Literal(l) => {
            let s = l.to_string();
            if s.starts_with('"') {
                syn::parse_str::<syn::LitStr>(&s).ok().map(|ls| ls.value())
            } else {
                None
            }
        }
        _ => None,
    })
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
