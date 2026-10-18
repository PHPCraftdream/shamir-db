//! Per-file AST rules of the `structure` gate:
//!
//! * `MODRS_LOGIC` — a `mod.rs` holds only attrs/docs, `mod` decls and
//!   `use`/`pub use` re-exports.
//! * `TESTS_MANIFEST` — a `tests/mod.rs` is a pure `mod` manifest.
//! * `INLINE_TEST_MOD` — an inline test-only (or loom) module with bodies
//!   in a production file.
//! * `LOCAL_USE` — a function- or block-local `use`, outside the
//!   documented exceptions.

use crate::exceptions::Exceptions;
use crate::finding::Report;
use crate::gitfs::GitFs;
use proc_macro2::Span;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::{File, Item, ItemMod, Stmt};

use super::owned;

/// Checks rules 1–4 and 5 (via `owned`) over every selected file.
pub fn check_files(
    fs: &GitFs,
    files: &[String],
    exceptions: &Exceptions,
    owned_cfg: &owned::OwnedImplConfig,
    report: &mut Report,
) {
    for path in files {
        let Some(src) = fs.read(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else {
            report.add(path, 1, "PARSE_FAILED", "cannot parse file".into());
            continue;
        };
        let lines: Vec<String> = src.lines().map(str::to_owned).collect();
        check_mod_rs(path, &file, report);
        check_tests_manifest(path, &file, report);
        check_inline_tests(path, &file, exceptions, report);
        check_local_uses(path, &file, &lines, report);
        owned::check_file(path, &file, owned_cfg, report);
    }
}

/// Rule 1: `mod.rs` may contain `mod` declarations and `use` re-exports
/// only (attributes and doc comments live on those items).
fn check_mod_rs(path: &str, file: &File, report: &mut Report) {
    if !(path.ends_with("/mod.rs") || path == "mod.rs") {
        return;
    }
    for item in &file.items {
        let line = line_of(item.span());
        match item {
            Item::Mod(_) | Item::Use(_) => {}
            _ => report.add(
                path,
                line,
                "MODRS_LOGIC",
                format!(
                    "mod.rs holds `{}`; only `mod` declarations and `use` re-exports are allowed",
                    kind_name(item)
                ),
            ),
        }
    }
}

/// Rule 2: `tests/mod.rs` is a pure manifest — `mod` declarations only,
/// not even `use`.
fn check_tests_manifest(path: &str, file: &File, report: &mut Report) {
    if path.rsplit('/').take(2).nth(1) != Some("tests") || !path.ends_with("mod.rs") {
        return;
    }
    for item in &file.items {
        if let Item::Mod(m) = item {
            if m.content.is_none() {
                continue;
            }
        }
        report.add(
            path,
            line_of(item.span()),
            "TESTS_MANIFEST",
            format!(
                "tests/mod.rs must be a pure `mod` manifest; found `{}`",
                kind_name(item)
            ),
        );
    }
}

/// Rule 3: inline test-only modules (including loom models) with bodies
/// in production files, unless the file is listed in `[[inline_tests]]`.
fn check_inline_tests(path: &str, file: &File, exceptions: &Exceptions, report: &mut Report) {
    for item in &file.items {
        walk_inline_tests(path, item, exceptions, report);
    }
}

fn walk_inline_tests(path: &str, item: &Item, exceptions: &Exceptions, report: &mut Report) {
    if let Item::Mod(m) = item {
        let name = m.ident.to_string();
        let line = line_of(m.mod_token.span());
        if is_inline_test_mod(m) {
            let has_bodies = m
                .content
                .as_ref()
                .map(|(_, items)| !items.is_empty())
                .unwrap_or(false);
            if has_bodies {
                if let Some(exc) = exceptions.inline_tests_for(path) {
                    eprintln!(
                        "INFO: {path}:{line}: inline test mod `{name}` accepted by exception ({}: {})",
                        exc.owner, exc.reason
                    );
                    return;
                }
                let why = if name == "loom" || name.ends_with("_loom") {
                    "loom model module declared inline"
                } else {
                    "inline test module with bodies in a production file"
                };
                report.add(
                    path,
                    line,
                    "INLINE_TEST_MOD",
                    format!("{why}: move `mod {name}` to its own file"),
                );
            }
        }
        if let Some((_, items)) = &m.content {
            for inner in items {
                walk_inline_tests(path, inner, exceptions, report);
            }
        }
    }
}

/// True for an inline `mod` that the guard treats as test-only: a `cfg`
/// mentioning `test` or `loom`, a mod named `tests` carrying any `cfg`,
/// a `*_tests` mod, or a `loom`/`*_loom` model module.
fn is_inline_test_mod(m: &ItemMod) -> bool {
    let name = m.ident.to_string();
    let cfgs: Vec<String> = m
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg"))
        .map(|a| a.to_token_stream().to_string())
        .collect();
    if cfgs
        .iter()
        .any(|c| c.contains("test") || c.contains("loom"))
    {
        return true;
    }
    if name == "tests" && !cfgs.is_empty() {
        return true;
    }
    name.ends_with("_tests") || name == "loom" || name.ends_with("_loom")
}

// ---------- LOCAL_USE ----------

/// Rule 4: function- or block-local `use` statements outside the
/// documented exceptions (see README).
fn check_local_uses(path: &str, file: &File, lines: &[String], report: &mut Report) {
    let mut ctx = Ctx {
        path,
        lines,
        report,
        in_test_mod: false,
        cfg_item_depth: 0,
    };
    for item in &file.items {
        walk_item(&mut ctx, item);
    }
}

struct Ctx<'a> {
    path: &'a str,
    lines: &'a [String],
    report: &'a mut Report,
    in_test_mod: bool,
    /// Depth of enclosing items carrying a `cfg` attribute (their bodies
    /// are treated as macro/cfg-generated and skipped).
    cfg_item_depth: usize,
}

fn walk_item(ctx: &mut Ctx, item: &Item) {
    match item {
        Item::Mod(m) => walk_mod_item(ctx, m),
        Item::Fn(f) => {
            if f.attrs.iter().any(|a| a.path().is_ident("cfg")) {
                ctx.cfg_item_depth += 1;
            }
            walk_stmts(ctx, &f.block.stmts);
            if f.attrs.iter().any(|a| a.path().is_ident("cfg")) {
                ctx.cfg_item_depth -= 1;
            }
        }
        _ => {}
    }
}

fn walk_mod_item(ctx: &mut Ctx, m: &ItemMod) {
    if let Some((_, items)) = &m.content {
        let was_test = ctx.in_test_mod;
        let was_cfg = ctx.cfg_item_depth;
        if is_test_context_mod(m) {
            ctx.in_test_mod = true;
        }
        if m.attrs.iter().any(|a| a.path().is_ident("cfg")) {
            ctx.cfg_item_depth += 1;
        }
        for inner in items {
            walk_item(ctx, inner);
        }
        ctx.in_test_mod = was_test;
        ctx.cfg_item_depth = was_cfg;
    }
}

/// A module whose context exempts `use super::*;`: `cfg(test)`, a mod
/// named `tests`, or `*_tests`.
fn is_test_context_mod(m: &ItemMod) -> bool {
    let name = m.ident.to_string();
    m.attrs
        .iter()
        .any(|a| a.path().is_ident("cfg") && a.to_token_stream().to_string().contains("test"))
        || name == "tests"
        || name.ends_with("_tests")
}

fn walk_stmts(ctx: &mut Ctx, stmts: &[Stmt]) {
    for stmt in stmts {
        match stmt {
            Stmt::Item(Item::Use(u)) => check_local_use(ctx, u),
            Stmt::Item(inner) => walk_item(ctx, inner),
            Stmt::Expr(syn::Expr::Block(b), _) => walk_stmts(ctx, &b.block.stmts),
            _ => {}
        }
    }
}

fn check_local_use(ctx: &mut Ctx, u: &syn::ItemUse) {
    let line = line_of(u.span());
    let text = quote_use(u);
    // Exception 1: `use super::*;` inside a test-context module.
    if ctx.in_test_mod && text == "super :: *" {
        return;
    }
    // Exception 2: the enclosing item is cfg-gated (macro-generated or
    // conditionally compiled body).
    if ctx.cfg_item_depth > 0 {
        return;
    }
    // Exception 3: a "collision" comment on its own line directly above.
    if line >= 2 {
        if let Some(prev) = ctx.lines.get(line - 2) {
            let t = prev.trim_start();
            if t.starts_with("//") && t.to_ascii_lowercase().contains("collision") {
                return;
            }
        }
    }
    ctx.report.add(
        ctx.path,
        line,
        "LOCAL_USE",
        format!("function-local `use` ({text}); move imports to the top of the file"),
    );
}

/// Compact rendering of a `use` tree for messages and exception checks.
fn quote_use(u: &syn::ItemUse) -> String {
    let mut out = String::new();
    if u.vis != syn::Visibility::Inherited {
        out.push_str("pub ");
    }
    out.push_str("use ");
    out.push_str(&u.tree.to_token_stream().to_string());
    out.trim_end_matches(';').trim().to_owned()
}

fn kind_name(item: &Item) -> &'static str {
    match item {
        Item::Const(_) => "const",
        Item::Enum(_) => "enum",
        Item::ExternCrate(_) => "extern crate",
        Item::Fn(_) => "fn",
        Item::ForeignMod(_) => "extern block",
        Item::Impl(_) => "impl",
        Item::Macro(_) => "macro invocation",
        Item::Mod(_) => "mod",
        Item::Static(_) => "static",
        Item::Struct(_) => "struct",
        Item::Trait(_) => "trait",
        Item::TraitAlias(_) => "trait alias",
        Item::Type(_) => "type",
        Item::Union(_) => "union",
        Item::Use(_) => "use",
        _ => "item",
    }
}

/// Line number of a span (needs proc-macro2 `span-locations`).
fn line_of(span: Span) -> usize {
    span.start().line
}
