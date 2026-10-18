//! Rule 6 — visibility widening and new `#[allow(...)]` versus the base
//! revision (`--base`, or the merge base of `--changed-since`).
//!
//! * `VIS_WIDENING` — an item's visibility grew (private <
//!   `pub(super)`/`pub(in)` < `pub(crate)` < `pub`).
//! * `ALLOW_ADDED` — an `#[allow(...)]`/`#![allow(...)]` attribute that
//!   does not exist (textually, normalised) in the base file.
//!
//! Both are suppressed when the `--delta` file lists the path.

use super::select::Delta;
use crate::finding::Report;
use crate::gitfs::GitFs;
use proc_macro2::Span;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{File, Item, Visibility};

/// Runs rules 6 over every selected file present in both revisions.
pub fn check_files(
    head_fs: &GitFs,
    base_rev: &str,
    files: &[String],
    delta: &Delta,
    report: &mut Report,
) {
    let base_fs = match GitFs::open(Some(base_rev), head_fs.root()) {
        Ok(fs) => fs,
        Err(e) => {
            eprintln!("rust-guard: warning: cannot open base {base_rev}: {e}");
            return;
        }
    };
    for path in files {
        let (Some(head_src), Some(base_src)) = (head_fs.read(path), base_fs.read(path)) else {
            continue; // new or deleted file: nothing to compare
        };
        let (Ok(head), Ok(base)) = (syn::parse_file(&head_src), syn::parse_file(&base_src)) else {
            continue; // unparseable revisions are reported by other rules
        };
        check_vis(path, &base, &head, delta, report);
        check_allows(path, &base, &head, delta, report);
    }
}

/// Ordered visibility levels for the widening comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum VisRank {
    Private,
    /// `pub(super)`/`pub(in)` and `pub(crate)`: one restricted tier.
    Restricted,
    Public,
}

fn rank(v: &Visibility) -> VisRank {
    match v {
        Visibility::Inherited => VisRank::Private,
        Visibility::Restricted(_) => VisRank::Restricted,
        Visibility::Public(_) => VisRank::Public,
    }
}

/// Item identity for cross-revision matching: top level items by
/// (kind, name), members of inherent impl blocks by (impl type, name).
#[derive(Debug, PartialEq, Eq, Hash)]
struct ItemKey {
    kind: &'static str,
    name: String,
}

/// Collects `key -> (visibility, line)` for one revision.
fn vis_index(file: &File) -> HashMap<ItemKey, (VisRank, usize)> {
    let mut out = HashMap::new();
    let add = |kind: &'static str,
               name: String,
               vis: &Visibility,
               span: Span,
               out: &mut HashMap<ItemKey, (VisRank, usize)>| {
        out.insert(ItemKey { kind, name }, (rank(vis), span.start().line));
    };
    for item in &file.items {
        match item {
            Item::Fn(i) => add("fn", i.sig.ident.to_string(), &i.vis, i.span(), &mut out),
            Item::Struct(i) => add("struct", i.ident.to_string(), &i.vis, i.span(), &mut out),
            Item::Enum(i) => add("enum", i.ident.to_string(), &i.vis, i.span(), &mut out),
            Item::Union(i) => add("union", i.ident.to_string(), &i.vis, i.span(), &mut out),
            Item::Trait(i) => add("trait", i.ident.to_string(), &i.vis, i.span(), &mut out),
            Item::Type(i) => add("type", i.ident.to_string(), &i.vis, i.span(), &mut out),
            Item::Const(i) => add("const", i.ident.to_string(), &i.vis, i.span(), &mut out),
            Item::Static(i) => add("static", i.ident.to_string(), &i.vis, i.span(), &mut out),
            Item::Mod(i) => add("mod", i.ident.to_string(), &i.vis, i.span(), &mut out),
            Item::Impl(i) => {
                // Inherent-impl members are matched per (impl type, name).
                let ty = impl_self_name(i);
                for member in &i.items {
                    match member {
                        syn::ImplItem::Fn(f) => out.insert(
                            ItemKey {
                                kind: "impl-fn",
                                name: format!("{ty}::{}", f.sig.ident),
                            },
                            (rank(&f.vis), f.span().start().line),
                        ),
                        syn::ImplItem::Const(c) => out.insert(
                            ItemKey {
                                kind: "impl-const",
                                name: format!("{ty}::{}", c.ident),
                            },
                            (rank(&c.vis), c.span().start().line),
                        ),
                        syn::ImplItem::Type(t) => out.insert(
                            ItemKey {
                                kind: "impl-type",
                                name: format!("{ty}::{}", t.ident),
                            },
                            (rank(&t.vis), t.span().start().line),
                        ),
                        _ => continue,
                    };
                }
            }
            _ => {}
        }
    }
    out
}

/// First segment of an impl self type ("" when unrecognised).
fn impl_self_name(i: &syn::ItemImpl) -> String {
    if let syn::Type::Path(p) = i.self_ty.as_ref() {
        p.path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default()
    } else {
        String::new()
    }
}

/// Rule 6a: report visibility growth between base and head.
fn check_vis(path: &str, base: &File, head: &File, delta: &Delta, report: &mut Report) {
    let base_idx = vis_index(base);
    let head_idx = vis_index(head);
    for (key, (head_rank, line)) in head_idx {
        let Some((base_rank, _)) = base_idx.get(&key) else {
            continue; // new item, not a widening
        };
        if head_rank > *base_rank {
            if let Some(entry) = delta.covers("VIS_WIDENING", path) {
                eprintln!(
                    "INFO: {path}:{line}: widening of `{}` accepted by delta ({}: {})",
                    entry.item, entry.owner, entry.reason
                );
                continue;
            }
            let from = vis_word(*base_rank);
            let to = vis_word(head_rank);
            report.add(
                path,
                line,
                "VIS_WIDENING",
                format!(
                    "`{}` widened from {from} to {to}; list the path in the delta file or narrow it",
                    key_label(&key)
                ),
            );
        }
    }
}

fn vis_word(r: VisRank) -> &'static str {
    match r {
        VisRank::Private => "private",
        VisRank::Restricted => "pub(super)/pub(in)/pub(crate)",
        VisRank::Public => "pub",
    }
}

fn key_label(key: &ItemKey) -> String {
    key.name.clone()
}

/// Rule 6b: `#[allow(...)]`/`#![allow(...)]` attributes present in head
/// but not in base (matched on normalised attribute text).
fn check_allows(path: &str, base: &File, head: &File, delta: &Delta, report: &mut Report) {
    let mut base_set: HashSet<u64> = HashSet::new();
    collect_allow_hashes(base, &mut base_set);
    let mut head_attrs: Vec<(String, usize)> = Vec::new();
    collect_allow_texts(head, &mut head_attrs);
    for (text, line) in head_attrs {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        if base_set.contains(&h.finish()) {
            continue;
        }
        if let Some(entry) = delta.covers("ALLOW_ADDED", path) {
            eprintln!(
                "INFO: {path}:{line}: new allow on `{}` accepted by delta ({}: {})",
                entry.item, entry.owner, entry.reason
            );
            continue;
        }
        report.add(
            path,
            line,
            "ALLOW_ADDED",
            format!("new `{text}` not present in base; list the path in the delta file"),
        );
    }
}

/// Normalised text of every outer/inner `allow` attribute in the file
/// (file-level and item-level).
fn collect_allow_texts(file: &File, out: &mut Vec<(String, usize)>) {
    let mut v = AllowVisitor { out: Vec::new() };
    v.visit_file(file);
    out.append(&mut v.out);
}

fn collect_allow_hashes(file: &File, out: &mut HashSet<u64>) {
    let mut texts = Vec::new();
    collect_allow_texts(file, &mut texts);
    for (text, _) in texts {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        out.insert(h.finish());
    }
}

/// Visits every attribute in the tree, keeping `allow` ones.
struct AllowVisitor {
    out: Vec<(String, usize)>,
}

impl<'ast> Visit<'ast> for AllowVisitor {
    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        if attr.path().is_ident("allow") {
            self.out
                .push((normalise_attr(attr), attr.span().start().line));
        }
    }
}

/// Normalises an attribute to a compact, span-independent string.
fn normalise_attr(attr: &syn::Attribute) -> String {
    let prefix = if matches!(attr.style, syn::AttrStyle::Inner(_)) {
        "#!"
    } else {
        "#"
    };
    let args = attr
        .parse_args_with(|input: syn::parse::ParseStream| input.parse::<proc_macro2::TokenStream>())
        .map(|ts| ts.to_string())
        .unwrap_or_default();
    format!("{prefix}[allow({args})]")
}
