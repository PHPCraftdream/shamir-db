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
use std::collections::{HashMap, HashSet};
use std::path::Path;
use syn::spanned::Spanned;
use syn::{File, Item, ItemMacro, ItemMod};

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
    /// Directory context for non-`#[path]` `mod x;` resolution.
    mod_dir: String,
}

/// Shared state for one gate run.
struct Walker<'a> {
    fs: &'a GitFs,
    exc: &'a Exceptions,
    report: Report,
    /// Physical path -> logical paths that reached it.
    visited: HashMap<String, Vec<String>>,
    /// Exceptions that were actually needed (stale check).
    used: HashSet<String>,
    /// Producer names that actually suppressed a finding.
    used_producers: HashSet<String>,
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
    let roots = roots::discover(&fs)?;
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
        visited: HashMap::new(),
        used: HashSet::new(),
        used_producers: HashSet::new(),
    };

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
            mod_dir: resolution::mod_dir_for(&root.path, true),
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
            .push(ctx.logical.clone());
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
        let mod_dir = &ctx.mod_dir;
        for item in &file.items {
            match item {
                Item::Mod(m) => self.handle_mod(ctx, mod_dir, m),
                Item::Macro(mac) => self.handle_macro(ctx, mac),
                _ => {}
            }
        }
    }

    /// `mod x;` / inline `mod x { ... }`, cfg predicate carried along.
    fn handle_mod(&mut self, ctx: &Ctx, mod_dir: &str, m: &ItemMod) {
        let name = m.ident.to_string();
        let line = line_of(m.mod_token.span());
        let cfg = attr_cfg(&m.attrs);
        if cfg
            .as_deref()
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
        let path_attr = attr_path(&m.attrs).or_else(|| attr_cfg_attr_path(&m.attrs));
        match resolution::resolve(self.fs, &ctx.file, &name, mod_dir, path_attr.as_deref()) {
            Resolution::Found(child) => {
                if let Some(content) = &m.content {
                    self.walk_inline(ctx, &name, &content.1);
                } else {
                    let mod_dir = resolution::mod_dir_for(&child, false);
                    let child = Ctx {
                        file: child,
                        is_root: false,
                        logical: format!("{}::{name}", ctx.logical),
                        mod_dir,
                    };
                    self.walk_file(&child);
                }
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

    /// Inline `mod x { ... }`: same physical file, logical path extended,
    /// and the module name APPENDED to the resolution directory context
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
            mod_dir,
        };
        for item in content {
            match item {
                Item::Mod(m) => self.handle_mod(&inner, &inner.mod_dir, m),
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
            if self.exc.has_producer(&name) {
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

    /// `include!("literal")` of a tracked `.rs` file is a source edge.
    fn handle_include(&mut self, ctx: &Ctx, mac: &ItemMacro, line: usize) {
        let Some(lit) = first_string_literal(&mac.mac.tokens) else {
            return;
        };
        match resolution::resolve_include(self.fs, &ctx.file, &lit) {
            Resolution::Found(target) => {
                if target.ends_with(".rs") {
                    let mod_dir = resolution::mod_dir_for(&target, false);
                    let child = Ctx {
                        file: target,
                        is_root: false,
                        logical: format!("{}::include({lit})", ctx.logical),
                        mod_dir,
                    };
                    self.walk_file(&child);
                }
            }
            Resolution::Generated(raw) => {
                if self.exc.has_producer("include") {
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
    /// duplicate unless an exception covers it.
    fn check_duplicates(&mut self) {
        let mut findings: Vec<(String, usize, String)> = Vec::new();
        for (path, logicals) in &self.visited {
            let mut distinct: Vec<&String> = logicals.iter().collect();
            distinct.sort();
            distinct.dedup();
            if distinct.len() > 1 {
                if self.exc.by_path(path).is_some() {
                    self.used.insert(path.clone());
                } else {
                    findings.push((
                        path.clone(),
                        1,
                        format!(
                            "reached by {} logical paths: {}",
                            distinct.len(),
                            distinct
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(" | ")
                        ),
                    ));
                }
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

/// Extracts a `#[cfg(...)]` predicate as text, if present.
fn attr_cfg(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find(|a| a.path().is_ident("cfg")).map(|a| {
        a.parse_args_with(|input: syn::parse::ParseStream| {
            input.parse::<proc_macro2::TokenStream>()
        })
        .map(|ts| ts.to_string())
        .unwrap_or_default()
    })
}

/// Extracts a literal `#[path = "..."]`.
fn attr_path(attrs: &[syn::Attribute]) -> Option<String> {
    attrs
        .iter()
        .find(|a| a.path().is_ident("path"))
        .and_then(|a| a.parse_args::<syn::LitStr>().ok())
        .map(|l| l.value())
}

/// Extracts `#[cfg_attr(pred, path = "...")]`; skipped when `pred` is
/// statically unsatisfiable.
fn attr_cfg_attr_path(attrs: &[syn::Attribute]) -> Option<String> {
    for a in attrs.iter().filter(|a| a.path().is_ident("cfg_attr")) {
        let Ok(ts) = a.parse_args_with(|input: syn::parse::ParseStream| {
            input.parse::<proc_macro2::TokenStream>()
        }) else {
            continue;
        };
        let trees: Vec<proc_macro2::TokenTree> = ts.into_iter().collect();
        let pred: String = trees
            .iter()
            .take_while(|t| !matches!(t, proc_macro2::TokenTree::Punct(p) if p.as_char() == ','))
            .map(|t| t.to_string())
            .collect();
        if resolution::cfg_unsatisfiable(&pred) {
            continue;
        }
        for (i, t) in trees.iter().enumerate() {
            if let proc_macro2::TokenTree::Ident(id) = t {
                if id != "path" {
                    continue;
                }
                if let (
                    Some(proc_macro2::TokenTree::Punct(p)),
                    Some(proc_macro2::TokenTree::Literal(l)),
                ) = (trees.get(i + 1), trees.get(i + 2))
                {
                    if p.as_char() == '=' {
                        if let Ok(s) = syn::parse_str::<syn::LitStr>(&l.to_string()) {
                            return Some(s.value());
                        }
                    }
                }
            }
        }
    }
    None
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

/// True when the token stream mentions the `mod` keyword at all.
fn mentions_mod(tokens: &proc_macro2::TokenStream) -> bool {
    tokens.clone().into_iter().any(|t| match t {
        proc_macro2::TokenTree::Ident(id) => id == "mod",
        _ => false,
    })
}

/// Line number of a span (needs proc-macro2 `span-locations`).
fn line_of(span: Span) -> usize {
    span.start().line
}
