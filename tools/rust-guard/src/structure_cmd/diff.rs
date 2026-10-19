//! Rule 6 — visibility widening and new `#[allow(...)]` versus the base
//! revision (`--base`, or the merge base of `--changed-since`).
//!
//! * `VIS_WIDENING` — an item's visibility grew (private <
//!   `pub(self)`/`pub(super)`/`pub(in)` < `pub(crate)` < `pub`),
//!   including a brand-new file introducing `pub(crate)`+ items.
//! * `ALLOW_ADDED` — an `#[allow(...)]`/`#![allow(...)]` attribute that
//!   does not exist (textually, normalised) in the base file.
//!
//! Both are suppressed when the `--delta` file lists the path.

use super::select::Delta;
use crate::finding::Report;
use crate::gitfs::GitFs;
use proc_macro2::Span;
use std::collections::{BTreeMap, BTreeSet};
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
        let (head_src, base_src) = (head_fs.read(path), base_fs.read(path));
        match (head_src, base_src) {
            (Some(head_src), Some(base_src)) => {
                let (Ok(head), Ok(base)) = (syn::parse_file(&head_src), syn::parse_file(&base_src))
                else {
                    continue; // unparseable revisions are reported by other rules
                };
                check_vis(path, &base, &head, delta, report);
                check_allows(path, &base, &head, delta, report);
            }
            (Some(head_src), None) => {
                // New file: nothing to diff, but a brand-new file
                // introducing `pub(crate)`+ items is still a widening
                // (finding 10c).
                if let Ok(head) = syn::parse_file(&head_src) {
                    check_new_file(path, &head, delta, report);
                }
            }
            _ => continue, // deleted file or unparseable head: other rules report
        }
    }
}

/// Ordered visibility levels for the widening comparison. `pub(self)`,
/// `pub(super)` and `pub(in path)` share one tier: each confines the item
/// to an ancestor of the defining module, so `pub(super) -> pub(crate)`
/// still moves up a tier (finding 10a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum VisRank {
    Private,
    /// `pub(self)`/`pub(super)`/`pub(in path)`: confined to an ancestor
    /// of the defining module.
    RestrictedSuper,
    /// `pub(crate)`: visible anywhere inside the crate.
    RestrictedCrate,
    Public,
}

fn rank(v: &Visibility) -> VisRank {
    match v {
        Visibility::Inherited => VisRank::Private,
        Visibility::Restricted(r) => {
            // `pub(crate)` is syntactically a restricted vis whose path
            // is exactly `crate`.
            if r.path.is_ident("crate") {
                VisRank::RestrictedCrate
            } else {
                VisRank::RestrictedSuper
            }
        }
        Visibility::Public(_) => VisRank::Public,
    }
}

/// Item identity for cross-revision matching: top level items by
/// (kind, name), members of inherent impl blocks by (impl type, name).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ItemKey {
    kind: &'static str,
    name: String,
}

/// Collects `key -> (visibility, line)` for one revision. Recurses into
/// INLINE modules: the key carries the module path so base and head
/// entries match across revisions (finding 10b). Also covers struct
/// fields (`Struct::field`): a private field becoming `pub`/`pub(crate)`
/// is a widening.
fn vis_index(file: &File) -> BTreeMap<ItemKey, (VisRank, usize)> {
    let mut out = BTreeMap::new();
    collect_vis(&file.items, &[], &mut out);
    out
}

fn collect_vis(items: &[Item], prefix: &[String], out: &mut BTreeMap<ItemKey, (VisRank, usize)>) {
    let add = |kind: &'static str,
               name: String,
               vis: &Visibility,
               span: Span,
               out: &mut BTreeMap<ItemKey, (VisRank, usize)>| {
        out.insert(ItemKey { kind, name }, (rank(vis), span.start().line));
    };
    for item in items {
        match item {
            Item::Fn(i) => add("fn", qualified(prefix, &i.sig.ident), &i.vis, i.span(), out),
            Item::Struct(i) => {
                let name = qualified(prefix, &i.ident);
                add("struct", name.clone(), &i.vis, i.span(), out);
                for field in i.fields.iter() {
                    let Some(ident) = &field.ident else { continue };
                    add(
                        "field",
                        format!("{name}::{}", ident),
                        &field.vis,
                        field.span(),
                        out,
                    );
                }
            }
            Item::Enum(i) => add("enum", qualified(prefix, &i.ident), &i.vis, i.span(), out),
            Item::Union(i) => add("union", qualified(prefix, &i.ident), &i.vis, i.span(), out),
            Item::Trait(i) => add("trait", qualified(prefix, &i.ident), &i.vis, i.span(), out),
            Item::Type(i) => add("type", qualified(prefix, &i.ident), &i.vis, i.span(), out),
            Item::Const(i) => add("const", qualified(prefix, &i.ident), &i.vis, i.span(), out),
            Item::Static(i) => add("static", qualified(prefix, &i.ident), &i.vis, i.span(), out),
            Item::Mod(i) => {
                let name = qualified(prefix, &i.ident);
                add("mod", name.clone(), &i.vis, i.span(), out);
                if let Some((_, inner)) = &i.content {
                    collect_vis(inner, &[name], out);
                }
            }
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
}

fn qualified(prefix: &[String], ident: &syn::Ident) -> String {
    let mut out = prefix.join("::");
    if !out.is_empty() {
        out.push_str("::");
    }
    out.push_str(&ident.to_string());
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
        VisRank::RestrictedSuper => "pub(super)/pub(in)",
        VisRank::RestrictedCrate => "pub(crate)",
        VisRank::Public => "pub",
    }
}

/// Rule 6c (finding 10c): a file that does not exist in base is skipped
/// by the pairwise diff — but a brand-new file introducing `pub(crate)`
/// or `pub` items widens the crate's surface just the same. Items that
/// are private or confined to an ancestor module are fine.
fn check_new_file(path: &str, head: &File, delta: &Delta, report: &mut Report) {
    let idx = vis_index(head);
    // One widening item per new file is enough to block the merge.
    let Some((key, (head_rank, line))) = idx
        .into_iter()
        .filter(|(k, _)| k.kind != "mod")
        .filter(|(_, (r, _))| *r >= VisRank::RestrictedCrate)
        .min_by_key(|(_, (r, l))| (*r, *l))
    else {
        return;
    };
    if let Some(entry) = delta.covers("VIS_WIDENING", path) {
        eprintln!(
            "INFO: {path}:{line}: new-file widening of `{}` accepted by delta ({}: {})",
            entry.item, entry.owner, entry.reason
        );
        return;
    }
    let to = vis_word(head_rank);
    report.add(
        path,
        line,
        "VIS_WIDENING",
        format!(
            "new file introduces `{}` at {to} visibility; list the path in the delta file or narrow it",
            key_label(&key)
        ),
    );
}

fn key_label(key: &ItemKey) -> String {
    key.name.clone()
}

/// Rule 6b: `#[allow(...)]`/`#![allow(...)]` attributes present in head
/// but not in base (matched on normalised attribute text).
fn check_allows(path: &str, base: &File, head: &File, delta: &Delta, report: &mut Report) {
    let mut base_set: BTreeSet<u64> = BTreeSet::new();
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

fn collect_allow_hashes(file: &File, out: &mut BTreeSet<u64>) {
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
