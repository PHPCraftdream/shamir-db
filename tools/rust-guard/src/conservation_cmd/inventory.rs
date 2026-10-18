//! Per-revision inventory construction for the `conservation` gate.
//!
//! Reads only immutable git objects: `GitFs::list_tree` (via
//! `git ls-tree -r -z --name-only <rev>`) bounds the file set and
//! `GitFs::read` (`git show <rev>:<path>`) provides the bytes — the
//! working tree is never consulted. Package/target roots come from
//! `modules_cmd::roots` (cargo metadata members plus workspace-excluded
//! packages; `vendor/*` skipped), optionally narrowed by `--packages`.

use super::item::{Extractor, Item};
use crate::gitfs::GitFs;
use crate::modules_cmd::resolution;
use crate::modules_cmd::roots::{self, RootKind};
use syn::Item as SynItem;

/// Builds the inventory of all `.rs` items reachable from the target
/// roots of `rev`, restricted to `packages` (all when empty).
pub fn build(rev: &str, packages: &[String], repo: &std::path::Path) -> Result<Vec<Item>, String> {
    let fs = GitFs::open(Some(rev), repo)?;
    // Fail early when the revision does not name a commit.
    if fs.commit_sha(rev).is_none() {
        return Err(format!("BLOCKED_COMPARISON: `{rev}` is not a commit"));
    }
    let tree: Vec<String> = fs
        .list_tree()?
        .into_iter()
        .filter(|p| {
            p.ends_with(".rs")
                && !p.starts_with("vendor/")
                // Self-test fixtures are mounted by the test harness, not
                // part of the build — skip them (mirrors check_layout.py).
                && !crate::modules_cmd::has_excluded_dir(p)
        })
        .collect();
    let all_roots = roots::discover(&fs)?;
    let mut out: Vec<Item> = Vec::new();
    for root in &all_roots {
        if !packages.is_empty() && !packages.contains(&root.package) {
            continue;
        }
        if !tree.contains(&root.path) {
            continue;
        }
        walk_root(
            &fs,
            &tree,
            root.package.as_str(),
            &root.path,
            root.kind,
            &mut out,
        );
    }
    Ok(out)
}

/// Walks one target root, following `mod x;` / inline `mod x { }` and
/// recording every item with its logical path.
fn walk_root(
    fs: &GitFs,
    tree: &[String],
    package: &str,
    root_path: &str,
    kind: RootKind,
    out: &mut Vec<Item>,
) {
    let logical = format!("{package}::{}", kind_prefix(kind));
    walk_file(fs, tree, package, root_path, true, &logical, out);
}

fn kind_prefix(kind: RootKind) -> &'static str {
    match kind {
        RootKind::Lib => "lib",
        RootKind::Bin => "bin",
        RootKind::Test => "test",
        RootKind::Bench => "bench",
        RootKind::Example => "example",
        RootKind::Build => "build",
    }
}

/// Recursive file walk. `mod_dir` follows `resolution::mod_dir_for`.
fn walk_file(
    fs: &GitFs,
    tree: &[String],
    package: &str,
    file: &str,
    is_root: bool,
    logical: &str,
    out: &mut Vec<Item>,
) {
    let Some(source) = fs.read(file) else {
        return; // unreachable in practice: bounded by the tree listing
    };
    let Ok(parsed) = syn::parse_file(&source) else {
        return; // parse failures are surfaced by the `modules` gate
    };
    let ext = Extractor {
        package,
        path: file,
        logical_prefix: logical.to_owned(),
    };
    ext.extract(&parsed.items, out);
    let mod_dir = resolution::mod_dir_for(file, is_root);
    for item in &parsed.items {
        if let SynItem::Mod(m) = item {
            let Some((_, inner)) = &m.content else {
                let name = m.ident.to_string();
                let path_attr = mod_path_attr(&m.attrs);
                if let resolution::Resolution::Found(child) =
                    resolution::resolve(fs, file, &name, &mod_dir, path_attr.as_deref())
                {
                    if tree.contains(&child) {
                        walk_file(
                            fs,
                            tree,
                            package,
                            &child,
                            false,
                            &format!("{logical}::{name}"),
                            out,
                        );
                    }
                }
                continue;
            };
            let inner_ext = Extractor {
                package,
                path: file,
                logical_prefix: format!("{logical}::{}", m.ident),
            };
            inner_ext.extract(inner, out);
        }
    }
}

/// Literal `#[path = "..."]` (cfg_attr variants are handled by the
/// `modules` gate; conservation keeps to the literal form).
fn mod_path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs
        .iter()
        .find(|a| a.path().is_ident("path"))
        .and_then(|a| a.parse_args::<syn::LitStr>().ok())
        .map(|l| l.value())
}
