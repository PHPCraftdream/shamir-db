//! Per-file AST rules of the `structure` gate:
//!
//! * `MODRS_LOGIC` — a `mod.rs` holds only attrs/docs, `mod` decls and
//!   `use`/`pub use` re-exports.
//! * `TESTS_MANIFEST` — a `tests/mod.rs` is a pure `mod` manifest.
//! * `INLINE_TEST_MOD` — an inline test-only (or loom) module with bodies
//!   in a production file.
//! * `LOCAL_USE` — a function- or block-local `use`, outside the
//!   documented exceptions.

use crate::common::exceptions::Exceptions;
use crate::common::finding::Report;
use crate::common::gitfs::GitFs;
use proc_macro2::Span;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{File, Item, ItemMod};

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
        // `ReadError::Missing` remains a silent skip for selected paths removed
        // from the working tree; `ReadError::Io` (including invalid UTF-8)
        // must surface as UNREADABLE_FILE rather than disappearing from every rule.
        let src = match fs.read_checked(path) {
            Err(crate::common::gitfs::ReadError::Missing) => continue,
            Err(e @ crate::common::gitfs::ReadError::Io(_)) => {
                report.add(
                    path,
                    1,
                    "UNREADABLE_FILE",
                    format!("cannot read selected file: {e}"),
                );
                continue;
            }
            Ok(src) => src,
        };
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
/// only (attributes and doc comments live on those items). INLINE
/// modules inside the mod.rs obey the same rule recursively (finding
/// 10f): `mod x { pub fn f() {} }` hides logic just as well at the top
/// level.
fn check_mod_rs(path: &str, file: &File, report: &mut Report) {
    if !(path.ends_with("/mod.rs") || path == "mod.rs") {
        return;
    }
    check_mod_rs_items(path, &file.items, report);
}

fn check_mod_rs_items(path: &str, items: &[Item], report: &mut Report) {
    for item in items {
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
        if let Item::Mod(m) = item {
            if let Some((_, inner)) = &m.content {
                check_mod_rs_items(path, inner, report);
            }
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
    let mut v = LocalUseVisitor {
        path,
        lines,
        report,
        in_test_mod: false,
        cfg_item_depth: 0,
        item_depth: 0usize,
    };
    v.visit_file(file);
}

/// A full-file `syn::visit::Visit` walk (finding 10e): a local `use` is
/// any `Item::Use` nested inside another item — an impl method body, a
/// closure, a loop or a conditional, at any depth — not just directly in
/// a top-level function's statement list, which the previous hand-rolled
/// traversal covered.
struct LocalUseVisitor<'a, 'r> {
    path: &'a str,
    lines: &'a [String],
    report: &'r mut Report,
    in_test_mod: bool,
    /// Depth of enclosing items carrying a `cfg` attribute (their bodies
    /// are treated as macro/cfg-generated and skipped).
    cfg_item_depth: usize,
    /// How many items enclose the one currently visited (0 = file level).
    item_depth: usize,
}

impl<'ast> Visit<'ast> for LocalUseVisitor<'_, '_> {
    fn visit_item(&mut self, i: &'ast Item) {
        if let Item::Use(u) = i {
            // A file- or module-level `use` (item_depth == 0 for the top
            // level; module-level uses inside inline mods re-enter at 0
            // through their own visit_item) is fine; anything nested is
            // a function- or block-local `use`.
            if self.item_depth > 0 {
                self.check_local_use(u);
            }
            return; // `use` items have no nested items
        }
        let was_test = self.in_test_mod;
        let was_cfg = self.cfg_item_depth;
        if let Item::Mod(m) = i {
            if is_test_context_mod(m) {
                self.in_test_mod = true;
            }
        }
        if attrs_of(i).iter().any(|a| a.path().is_ident("cfg")) {
            self.cfg_item_depth += 1;
        }
        let was_depth = self.item_depth;
        if matches!(i, Item::Mod(_)) {
            // Items directly in a module body (top level or an inline
            // mod) are module-level: a `use` there is NOT local.
            self.item_depth = 0;
        } else {
            self.item_depth += 1;
        }
        visit::visit_item(self, i);
        self.item_depth = was_depth;
        self.in_test_mod = was_test;
        self.cfg_item_depth = was_cfg;
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

impl LocalUseVisitor<'_, '_> {
    fn check_local_use(&mut self, u: &syn::ItemUse) {
        let line = line_of(u.span());
        let text = quote_use(u);
        // Exception 1: `use super::*;` inside a test-context module.
        if self.in_test_mod && text == "super :: *" {
            return;
        }
        // Exception 2: the enclosing item is cfg-gated (macro-generated or
        // conditionally compiled body).
        if self.cfg_item_depth > 0 {
            return;
        }
        // Exception 3: a "collision" comment on its own line directly above.
        if line >= 2 {
            if let Some(prev) = self.lines.get(line - 2) {
                let t = prev.trim_start();
                if t.starts_with("//") && t.to_ascii_lowercase().contains("collision") {
                    return;
                }
            }
        }
        self.report.add(
            self.path,
            line,
            "LOCAL_USE",
            format!("function-local `use` ({text}); move imports to the top of the file"),
        );
    }
}

/// Outer attributes of any item kind.
fn attrs_of(item: &Item) -> &[syn::Attribute] {
    match item {
        Item::Const(x) => &x.attrs,
        Item::Enum(x) => &x.attrs,
        Item::ExternCrate(x) => &x.attrs,
        Item::Fn(x) => &x.attrs,
        Item::ForeignMod(x) => &x.attrs,
        Item::Impl(x) => &x.attrs,
        Item::Macro(x) => &x.attrs,
        Item::Mod(x) => &x.attrs,
        Item::Static(x) => &x.attrs,
        Item::Struct(x) => &x.attrs,
        Item::Trait(x) => &x.attrs,
        Item::TraitAlias(x) => &x.attrs,
        Item::Type(x) => &x.attrs,
        Item::Union(x) => &x.attrs,
        Item::Use(x) => &x.attrs,
        _ => &[],
    }
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
