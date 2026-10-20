//! Rule 6 — visibility widening and new `#[allow(...)]` versus the base
//! revision (`--base`, or the merge base of `--changed-since`).
//!
//! * `VIS_WIDENING` — an item's visibility grew (private <
//!   `pub(self)`/`pub(super)`/`pub(in)` < `pub(crate)` < `pub`),
//!   including a brand-new (or moved) file introducing `pub(crate)`+
//!   items.
//! * `ALLOW_ADDED` — an `#[allow(...)]`/`#![allow(...)]` attribute that
//!   does not exist (textually, normalised) in the base file.
//! * `UNUSED_ALLOWANCE` — a delta entry that matched no finding this
//!   run; stale entries must be pruned, never ignored.
//!
//! Suppression is per (path, item): a delta entry covers exactly one
//! item on exactly one path and is consumed on first use.

use super::select::Delta;
use crate::common::finding::Report;
use crate::common::gitfs::GitFs;
use proc_macro2::Span;
use quote::ToTokens;
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
    delta: &mut Delta,
    report: &mut Report,
) {
    let base_fs = match GitFs::open(Some(base_rev), head_fs.root()) {
        Ok(fs) => fs,
        Err(e) => {
            eprintln!("rust-guard: warning: cannot open base {base_rev}: {e}");
            return;
        }
    };
    // Lazily built index of the base tree's `.rs` files, used only to
    // pair moved files (see `find_moved`). A base file that STILL
    // exists at head is a COPY SOURCE, never a move source — it is
    // filtered out here, before the cache is even built.
    let base_tree = base_fs.list_tree().unwrap_or_default();
    let base_rs: Vec<String> = {
        let mut v: Vec<String> = base_tree
            .iter()
            .filter(|p| {
                p.ends_with(".rs")
                    // `head_fs` is `GitFs::open(None)`, so `exists` reads
                    // the real working tree — exactly what this gate
                    // scans. A base file still present at head stays out
                    // of the candidate list.
                    && !head_fs.exists(p)
            })
            .cloned()
            .collect();
        v.sort();
        v
    };
    let mut base_index: Option<Vec<BaseFile>> = None;
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
                let Ok(head) = syn::parse_file(&head_src) else {
                    continue;
                };
                // A file absent from base is new OR moved. A moved file
                // (content-identical to some base file, no widening) is
                // checked against its base counterpart, so a moved file
                // can still gain findings — it is NOT treated as new.
                match find_moved(&head, &mut base_index, &base_fs, &base_rs) {
                    Some(moved) => {
                        check_vis(path, &moved.file, &head, delta, report);
                        check_allows(path, &moved.file, &head, delta, report);
                    }
                    None => {
                        // Brand-new file: `pub(crate)`+ items widen the
                        // crate's surface (finding 10c), and every
                        // `#[allow]` it carries is new (empty base).
                        check_new_file(path, &head, delta, report);
                        let empty = syn::parse_file("").expect("empty file parses");
                        check_allows(path, &empty, &head, delta, report);
                    }
                }
            }
            _ => continue, // deleted file or unparseable head: other rules report
        }
    }
    report_unused(delta, report);
}

/// Reports every delta entry that matched nothing as `UNUSED_ALLOWANCE`.
/// Also used by `select::execute` when the diff never runs (no base, or
/// no files selected), so a non-empty delta can never pass silently.
pub(crate) fn report_unused(delta: &Delta, report: &mut Report) {
    for (code, e) in delta
        .allow_widen
        .iter()
        .map(|e| ("allow_widen", e))
        .chain(delta.allow.iter().map(|e| ("allow", e)))
    {
        report.add(
            &e.path,
            1,
            "UNUSED_ALLOWANCE",
            format!(
                "delta [{code}] entry `{}` matched no finding this run; remove it or fix the item",
                e.item
            ),
        );
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
/// (kind, name), members of impl blocks by (impl type, name). Members
/// of inline modules carry the module path, so same-named impls in two
/// different inline modules never overwrite each other (R7c).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ItemKey {
    kind: &'static str,
    name: String,
}

/// One indexed item: visibility rank, source line and a body
/// fingerprint used for moved-file pairing.
#[derive(Debug, Clone, Copy)]
struct Entry {
    rank: VisRank,
    line: usize,
    /// Span-independent hash of the item's tokens (outer attributes
    /// are STRIPPED before hashing, visibility INCLUDED — a widened
    /// item therefore does NOT pair as moved; it falls back to the
    /// new-file check, which reports the widening conservatively).
    fp: u64,
}

/// Per-revision index of one file.
type Index = BTreeMap<ItemKey, Entry>;

/// Collects `key -> entry` for one revision. Recurses into INLINE
/// modules: the key carries the module path so base and head entries
/// match across revisions (finding 10b). Also covers struct fields —
/// named (`Struct::field`) and tuple (`Struct::0`, `Struct::1`, ...)
/// alike (R7b) — and impl members of BOTH inherent and trait impls,
/// keyed with the inline-module prefix (R7c).
fn vis_index(file: &File) -> Index {
    let mut out = BTreeMap::new();
    collect_vis(&file.items, &[], &mut out);
    out
}

fn collect_vis(items: &[Item], prefix: &[String], out: &mut Index) {
    let add = |kind: &'static str,
               name: String,
               vis: &Visibility,
               span: Span,
               fp: u64,
               out: &mut Index| {
        out.insert(
            ItemKey { kind, name },
            Entry {
                rank: rank(vis),
                line: span.start().line,
                fp,
            },
        );
    };
    for item in items {
        match item {
            Item::Fn(i) => add(
                "fn",
                qualified(prefix, &i.sig.ident.to_string()),
                &i.vis,
                i.span(),
                hash_tokens(i),
                out,
            ),
            Item::Struct(i) => {
                let name = qualified(prefix, &i.ident.to_string());
                add(
                    "struct",
                    name.clone(),
                    &i.vis,
                    i.span(),
                    hash_tokens(i),
                    out,
                );
                for (idx, field) in i.fields.iter().enumerate() {
                    let member = match &field.ident {
                        Some(ident) => ident.to_string(),
                        None => idx.to_string(),
                    };
                    add(
                        "field",
                        format!("{name}::{member}"),
                        &field.vis,
                        field.span(),
                        hash_tokens(field),
                        out,
                    );
                }
            }
            Item::Enum(i) => add(
                "enum",
                qualified(prefix, &i.ident.to_string()),
                &i.vis,
                i.span(),
                hash_tokens(i),
                out,
            ),
            Item::Union(i) => add(
                "union",
                qualified(prefix, &i.ident.to_string()),
                &i.vis,
                i.span(),
                hash_tokens(i),
                out,
            ),
            Item::Trait(i) => add(
                "trait",
                qualified(prefix, &i.ident.to_string()),
                &i.vis,
                i.span(),
                hash_tokens(i),
                out,
            ),
            Item::Type(i) => add(
                "type",
                qualified(prefix, &i.ident.to_string()),
                &i.vis,
                i.span(),
                hash_tokens(i),
                out,
            ),
            Item::Const(i) => add(
                "const",
                qualified(prefix, &i.ident.to_string()),
                &i.vis,
                i.span(),
                hash_tokens(i),
                out,
            ),
            Item::Static(i) => add(
                "static",
                qualified(prefix, &i.ident.to_string()),
                &i.vis,
                i.span(),
                hash_tokens(i),
                out,
            ),
            Item::Mod(i) => {
                let name = qualified(prefix, &i.ident.to_string());
                add("mod", name.clone(), &i.vis, i.span(), hash_tokens(i), out);
                if let Some((_, inner)) = &i.content {
                    collect_vis(inner, &[name], out);
                }
            }
            Item::Impl(i) => {
                // Impl members (inherent AND trait impls) are matched
                // per (prefix, impl self type, name) so that two
                // same-named impls in two inline modules stay distinct
                // (R7c).
                let ty = qualified(prefix, &impl_self_name(i));
                for member in &i.items {
                    let (kind, ident, vis, span) = match member {
                        syn::ImplItem::Fn(f) => {
                            ("impl-fn", f.sig.ident.to_string(), &f.vis, f.span())
                        }
                        syn::ImplItem::Const(c) => {
                            ("impl-const", c.ident.to_string(), &c.vis, c.span())
                        }
                        syn::ImplItem::Type(t) => {
                            ("impl-type", t.ident.to_string(), &t.vis, t.span())
                        }
                        _ => continue,
                    };
                    out.insert(
                        ItemKey {
                            kind,
                            name: format!("{ty}::{ident}"),
                        },
                        Entry {
                            rank: rank(vis),
                            line: span.start().line,
                            fp: hash_tokens(member),
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

fn qualified(prefix: &[String], ident: &str) -> String {
    let mut out = prefix.join("::");
    if !out.is_empty() {
        out.push_str("::");
    }
    out.push_str(ident);
    out
}

/// LAST segment of an impl self type path (`RepoInstance` from
/// `RepoInstance<..>`, `crate::fix::RepoInstance` or any other plain
/// path — the last segment is the type's own name); "" for other
/// shapes. `<T as Trait>::Assoc` self types (a path carrying a `QSelf`)
/// stay out of scope on purpose: their governing type is not a single
/// plain name, so ownership is undefined here.
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

/// Span-independent hash of a syntax node's token stream, with any
/// LEADING `#[...]` / `#![...]` outer-attribute groups stripped first
/// (syn's ToTokens prints attributes, so without stripping an added
/// `#[allow]` would change a fingerprint and break moved-file pairing);
/// visibility IS included, so a widened item never pairs as moved.
/// Used ONLY for the moved-file pairing fingerprint; `check_vis` and
/// friends compare visibility ranks, not fingerprints.
fn hash_tokens<T: ToTokens>(item: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    strip_outer_attrs(item.to_token_stream())
        .to_string()
        .hash(&mut h);
    h.finish()
}

/// Drops the leading `#[...]` / `#![...]` attribute token groups from a
/// token stream, keeping everything from the first non-attribute token
/// on. A `#` not shaped like an attribute is kept verbatim.
fn strip_outer_attrs(ts: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let mut iter = ts.into_iter().peekable();
    let mut consumed: Vec<proc_macro2::TokenTree> = Vec::new();
    loop {
        match iter.peek() {
            Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '#' => {
                consumed.push(iter.next().expect("peeked"));
                if matches!(
                    iter.peek(),
                    Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '!'
                ) {
                    consumed.push(iter.next().expect("peeked"));
                }
                match iter.peek() {
                    Some(proc_macro2::TokenTree::Group(_)) => {
                        iter.next();
                        consumed.clear();
                    }
                    _ => break,
                }
            }
            _ => break,
        }
    }
    let mut out: proc_macro2::TokenStream = consumed.into_iter().collect();
    out.extend(iter);
    out
}

/// A parsed base file, cached for moved-file pairing.
struct BaseFile {
    path: String,
    file: File,
    index: Index,
}

/// Moved-file pairing heuristic (R7d, documented): a head file absent
/// from base is "moved" when some base `.rs` file has the SAME multiset
/// of (kind, qualified name, body fingerprint) — fingerprints are
/// attribute-insensitive (see `hash_tokens`), so a moved file whose
/// only change is an added `#[allow]` still pairs; visibilities are
/// NOT compared here but checked afterwards by pairing the
/// file with a stricter condition: every head visibility must be
/// equal-or-narrower than its base counterpart (`check_vis` then finds
/// nothing and `check_allows` still polices new `#[allow]`s). If any
/// visibility widened, the file does NOT pair and falls back to the
/// new-file check, which reports the widening conservatively — a false
/// POSITIVE is impossible (pairing requires fingerprint equality);
/// a
/// widened-then-moved file simply gets the slightly blunter new-file
/// report. The first base candidate in sorted order wins.
///
/// A candidate is CONSUMED on pairing: the returned `BaseFile` is
/// removed from the cached list, so one base file can never be paired
/// as the move source of two different head files (and every candidate
/// is already absent at head — see the `base_rs` filter in
/// `check_files` — so a COPY of a base file never pairs).
fn find_moved(
    head: &File,
    cache: &mut Option<Vec<BaseFile>>,
    base_fs: &GitFs,
    base_rs: &[String],
) -> Option<BaseFile> {
    let list = cache.get_or_insert_with(|| {
        let mut v = Vec::new();
        for path in base_rs {
            let Some(src) = base_fs.read(path) else {
                continue;
            };
            let Ok(file) = syn::parse_file(&src) else {
                continue;
            };
            v.push(BaseFile {
                path: path.clone(),
                index: vis_index(&file),
                file,
            });
        }
        v
    });
    let head_idx = vis_index(head);
    let head_sigs: BTreeSet<(&str, &String, u64)> = head_idx
        .iter()
        .map(|(k, e)| (k.kind, &k.name, e.fp))
        .collect();
    let mut best: Option<usize> = None;
    for (i, b) in list.iter().enumerate() {
        let b_sigs: BTreeSet<(&str, &String, u64)> = b
            .index
            .iter()
            .map(|(k, e)| (k.kind, &k.name, e.fp))
            .collect();
        if b_sigs != head_sigs || b_sigs.is_empty() {
            continue;
        }
        // Every head visibility equal-or-narrower than base.
        if !head_idx
            .iter()
            .all(|(k, he)| b.index.get(k).map(|be| he.rank <= be.rank).unwrap_or(false))
        {
            continue;
        }
        best = Some(i);
        break;
    }
    best.map(|i| {
        // Consume the paired candidate (swap_remove keeps the cache
        // compact; candidate order is irrelevant — the first match wins
        // and is taken immediately).
        let b = list.swap_remove(i);
        eprintln!("INFO: paired with base `{}` as a moved file", b.path);
        b
    })
}

/// Rule 6a: report visibility growth between base and head.
fn check_vis(path: &str, base: &File, head: &File, delta: &mut Delta, report: &mut Report) {
    let base_idx = vis_index(base);
    let head_idx = vis_index(head);
    for (key, head_entry) in head_idx {
        let Some(base_entry) = base_idx.get(&key) else {
            continue; // new item, not a widening
        };
        if head_entry.rank > base_entry.rank {
            let label = key_label(&key);
            let line = head_entry.line;
            if delta.consume("VIS_WIDENING", path, &label) {
                eprintln!("INFO: {path}:{line}: widening of `{label}` accepted by delta");
                continue;
            }
            let from = vis_word(base_entry.rank);
            let to = vis_word(head_entry.rank);
            report.add(
                path,
                head_entry.line,
                "VIS_WIDENING",
                format!(
                    "`{label}` widened from {from} to {to}; add a delta entry naming this item or narrow it"
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

/// Rule 6c (finding 10c): a file that does not exist in base (and is
/// not a moved file) gets a per-item check: every item at
/// `pub(crate)`/`pub` visibility is a widening, matched against the
/// delta BY ITEM NAME — one entry no longer silences a whole file.
/// Items that are private or confined to an ancestor module are fine.
fn check_new_file(path: &str, head: &File, delta: &mut Delta, report: &mut Report) {
    let idx = vis_index(head);
    for (key, entry) in idx {
        if key.kind == "mod" || entry.rank < VisRank::RestrictedCrate {
            continue;
        }
        let label = key_label(&key);
        let line = entry.line;
        if delta.consume("VIS_WIDENING", path, &label) {
            eprintln!("INFO: {path}:{line}: new-file widening of `{label}` accepted by delta");
            continue;
        }
        let to = vis_word(entry.rank);
        report.add(
            path,
            entry.line,
            "VIS_WIDENING",
            format!(
                "new file introduces `{label}` at {to} visibility; add a delta entry naming this item or narrow it"
            ),
        );
    }
}

fn key_label(key: &ItemKey) -> String {
    key.name.clone()
}

/// Rule 6b: `#[allow(...)]`/`#![allow(...)]` attributes present in head
/// but not in base (matched on normalised attribute text, which is also
/// the delta item key for `ALLOW_ADDED`).
fn check_allows(path: &str, base: &File, head: &File, delta: &mut Delta, report: &mut Report) {
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
        if delta.consume("ALLOW_ADDED", path, &text) {
            eprintln!("INFO: {path}:{line}: new allow `{text}` accepted by delta");
            continue;
        }
        report.add(
            path,
            line,
            "ALLOW_ADDED",
            format!("new `{text}` not present in base; add a delta entry with this attribute text as item"),
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
