//! References from one production file to other modules of the same
//! crate: `use` items (grouped, renamed, glob, re-exported), paths in
//! code without `use`, `super::` chains that leave the module,
//! `crate::`-rooted paths inside macro token streams, and `impl`
//! self-types owned by another module. Test code is skipped and never
//! produces references.

use crate::commands::layers_cmd::graph::{
    absolute_base, cfg_mentions_test, expand_use_tree, is_test_name, item_attrs, CrateGraph,
    ModPath,
};
use proc_macro2::TokenTree;
use std::collections::BTreeMap;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

/// Per-file references: target module -> FIRST line of any reference.
pub(crate) struct FileRefs {
    pub(crate) targets: BTreeMap<ModPath, usize>,
}

/// Collects all same-crate module references in one production file.
pub(crate) fn collect(
    source: &str,
    logical: &ModPath,
    graph: &CrateGraph,
) -> Result<FileRefs, String> {
    let ast = syn::parse_file(source).map_err(|e| e.to_string())?;
    let mut visitor = RefVisitor {
        stack: logical.clone(),
        aliases: BTreeMap::new(),
        targets: BTreeMap::new(),
        graph,
    };
    visitor.visit_file(&ast);
    Ok(FileRefs {
        targets: visitor.targets,
    })
}

/// Syn visitor over one file.
struct RefVisitor<'a> {
    /// Logical module path of the code currently visited (inline `mod`
    /// blocks push their name).
    stack: ModPath,
    /// Local name -> canonical owner module, from use leaves in visit order.
    aliases: BTreeMap<String, ModPath>,
    targets: BTreeMap<ModPath, usize>,
    graph: &'a CrateGraph,
}

impl<'a> RefVisitor<'a> {
    /// Records a target, keeping the FIRST line seen.
    fn record(&mut self, target: ModPath, line: usize) {
        self.targets
            .entry(target)
            .and_modify(|l| {
                if line < *l {
                    *l = line;
                }
            })
            .or_insert(line);
    }

    /// Resolves segments rooted at the current stack position.
    fn resolve_rooted(&mut self, segs: &[String], line: usize) {
        let (base, rest) = absolute_base(&self.stack, segs);
        let target = self.graph.resolve_from(&base, &rest);
        if !target.is_empty() {
            self.record(target, line);
        }
    }
}

impl<'a> Visit<'a> for RefVisitor<'a> {
    /// Test-attributed items are skipped entirely (no descent).
    fn visit_item(&mut self, i: &'a syn::Item) {
        if cfg_mentions_test(item_attrs(i)) {
            return;
        }
        visit::visit_item(self, i);
    }

    fn visit_item_mod(&mut self, m: &'a syn::ItemMod) {
        if is_test_name(&m.ident.to_string()) || cfg_mentions_test(&m.attrs) {
            return;
        }
        let Some((_, items)) = &m.content else {
            // File-backed `mod x;` is a no-op: every file is visited once
            // via the graph's file list.
            return;
        };
        self.stack.push(m.ident.to_string());
        for item in items {
            self.visit_item(item);
        }
        self.stack.pop();
    }

    fn visit_item_use(&mut self, u: &'a syn::ItemUse) {
        if cfg_mentions_test(&u.attrs) {
            return;
        }
        let line = span_line(&u.span());
        let mut prefix = Vec::new();
        let mut leaves = Vec::new();
        expand_use_tree(&u.tree, &mut prefix, &mut leaves);
        for leaf in leaves {
            let base = absolute_base(&self.stack, &leaf.segs);
            let target = self.graph.resolve_from(&base.0, &base.1);
            if !target.is_empty() {
                self.record(target.clone(), line);
                if !leaf.local.is_empty() {
                    self.aliases.insert(leaf.local, target);
                }
            }
        }
    }

    fn visit_path(&mut self, p: &'a syn::Path) {
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        let line = span_line(&p.span());
        match segs.first().map(String::as_str) {
            Some("crate") | Some("self") | Some("super") => self.resolve_rooted(&segs, line),
            Some(first) => {
                if let Some(base) = self.aliases.get(first) {
                    let target = self.graph.resolve_from(base, &segs[1..]);
                    if !target.is_empty() {
                        self.record(target, line);
                    }
                }
                // Otherwise: external crate or a local item — unresolved.
            }
            None => {}
        }
        // Descend into generic arguments etc.
        visit::visit_path(self, p);
    }

    fn visit_macro(&mut self, m: &'a syn::Macro) {
        scan_macro_tokens(&m.tokens, self);
        visit::visit_macro(self, m);
    }

    fn visit_item_impl(&mut self, i: &'a syn::ItemImpl) {
        if cfg_mentions_test(&i.attrs) {
            return;
        }
        if let syn::Type::Path(tp) = i.self_ty.as_ref() {
            if tp.qself.is_none() {
                if let Some(last) = tp.path.segments.last() {
                    let ident = last.ident.to_string();
                    let line = span_line(&i.span());
                    for owner in self.graph.owner_modules(&ident) {
                        if owner != self.stack {
                            self.record(owner, line);
                        }
                    }
                }
            }
        }
        visit::visit_item_impl(self, i);
    }
}

/// Line number of a span (0 when unavailable).
fn span_line(span: &proc_macro2::Span) -> usize {
    span.start().line
}

/// Flattens a token stream (recursively through groups) in order, then
/// resolves every `crate :: ident (:: ident)*` and `super :: ident
/// (:: ident)*` chain found in it. The line is unknown inside a macro
/// expansion, so 0 is recorded (the file-level code paths carry exact
/// lines).
fn scan_macro_tokens(tokens: &proc_macro2::TokenStream, visitor: &mut RefVisitor<'_>) {
    fn walk(stream: &proc_macro2::TokenStream, out: &mut Vec<TokenKind>) {
        for tree in stream.clone() {
            match &tree {
                TokenTree::Group(g) => walk(&g.stream(), out),
                TokenTree::Ident(i) => out.push(TokenKind::Ident(i.to_string())),
                TokenTree::Punct(p) => out.push(TokenKind::Punct(p.as_char())),
                TokenTree::Literal(_) => out.push(TokenKind::Other),
            }
        }
    }
    let mut flat: Vec<TokenKind> = Vec::new();
    walk(tokens, &mut flat);
    let mut idx = 0;
    while idx + 1 < flat.len() {
        let starts_chain = matches!(&flat[idx], TokenKind::Ident(s) if s == "crate" || s == "super")
            && matches!(flat.get(idx + 1), Some(TokenKind::Punct(p)) if *p == ':');
        if !starts_chain {
            idx += 1;
            continue;
        }
        let root = match &flat[idx] {
            TokenKind::Ident(s) => s.clone(),
            _ => unreachable!(),
        };
        let mut segs = vec![root];
        let mut cursor = idx + 2;
        while cursor < flat.len() {
            match &flat[cursor] {
                TokenKind::Ident(s) => {
                    segs.push(s.clone());
                    cursor += 1;
                    // Continue only while `:: Ident` follows (stop cleanly
                    // at generics/other tokens).
                    if matches!(flat.get(cursor), Some(TokenKind::Punct(p)) if *p == ':')
                        && matches!(flat.get(cursor + 1), Some(TokenKind::Ident(_)))
                    {
                        cursor += 1;
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        visitor.resolve_rooted(&segs, 1);
        idx = cursor;
    }
}

/// Flattened token view used by the macro scanner.
enum TokenKind {
    Ident(String),
    Punct(char),
    Other,
}
