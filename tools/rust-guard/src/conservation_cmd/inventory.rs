//! Per-revision inventory construction for the `conservation` gate.
//!
//! Reads only immutable git objects: `GitFs::list_tree` (via
//! `git ls-tree -r -z --name-only <rev>`) bounds the file set and
//! `GitFs::read_checked` (`git show <rev>:<path>`) provides the bytes —
//! the working tree is never consulted, and no cargo is invoked: target
//! roots come from `roots::discover_from_manifests`, which parses the
//! pinned ROOT `Cargo.toml` ([workspace] members/exclude globs) and each
//! member manifest on the SAME pinned view (finding 7: the BASE
//! inventory must come from the BASE revision, so a target deleted at
//! head still yields its items as losses).
//!
//! Fail closed: an unreadable or unparseable source, or a manifest
//! problem, is collected into `Inventory::errors` and becomes an
//! `INVENTORY_ERROR` finding — never a silent skip.
//!
//! The walk follows `mod x;` declarations (including ones INSIDE inline
//! `mod x { }` blocks, with the extended directory context), literal
//! `include!("file.rs")` edges, and threads the inherited cfg predicate
//! down into every extracted item.

use super::item::{combine_cfg, Extractor, Item};
use crate::gitfs::GitFs;
use crate::modules_cmd::resolution::{self, Resolution};
use crate::modules_cmd::roots::{self, RootKind};
use crate::modules_cmd::scan_cfg::ScanCfg;
use syn::Item as SynItem;

/// Everything extracted from one revision: the items plus the fail-closed
/// problems (each rendered as an `INVENTORY_ERROR` finding by the caller).
pub struct Inventory {
    pub items: Vec<Item>,
    /// `(path, message)` pairs, one per unreadable/unparseable source or
    /// manifest problem.
    pub errors: Vec<(String, String)>,
}

/// Context of the file (or inline-module position) currently walked.
struct Ctx {
    file: String,
    is_root: bool,
    logical: String,
    package: String,
    /// Directory context for non-`#[path]` `mod x;` resolution.
    mod_dir: String,
    /// Base directory for `#[path]` resolution.
    attr_dir: String,
    /// cfg predicate inherited from the enclosing modules.
    cfg: Option<String>,
}

/// Builds the inventory of all `.rs` items reachable from the target
/// roots of `rev`, restricted to `packages` (all when empty).
pub fn build(rev: &str, packages: &[String], repo: &std::path::Path) -> Result<Inventory, String> {
    let fs = GitFs::open(Some(rev), repo)?;
    // Fail early when the revision does not name a commit.
    if fs.commit_sha(rev).is_none() {
        return Err(format!("BLOCKED_COMPARISON: `{rev}` is not a commit"));
    }
    // Repo-level scan configuration (working-tree state): excluded
    // prefixes are out of the inventory's scope, exactly as in the
    // `modules` gate.
    let scan = ScanCfg::load(repo)?;
    let tree: Vec<String> = fs
        .list_tree()?
        .into_iter()
        .filter(|p| {
            p.ends_with(".rs")
                && !p.starts_with("vendor/")
                // Self-test fixtures are mounted by the test harness, not
                // part of the build — skip them (mirrors check_layout.py).
                && !crate::modules_cmd::has_excluded_dir(p)
                && !scan.excluded(p)
        })
        .collect();
    let (all_roots, problems) = roots::discover_from_manifests(&fs)?;
    let mut inv = Inventory {
        items: Vec::new(),
        errors: problems
            .into_iter()
            .map(|p| (p.manifest, p.message))
            .collect(),
    };
    let mut visited: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for root in &all_roots {
        if !packages.is_empty() && !packages.contains(&root.package) {
            continue;
        }
        if !tree.contains(&root.path) {
            continue;
        }
        walk_root(&fs, &tree, root, &mut inv, &mut visited);
    }
    Ok(inv)
}

/// Walks one target root, following `mod x;` / inline `mod x { }` /
/// `include!` and recording every item with its logical path.
fn walk_root(
    fs: &GitFs,
    tree: &[String],
    root: &roots::TargetRoot,
    inv: &mut Inventory,
    visited: &mut std::collections::BTreeSet<String>,
) {
    let logical = format!("{}::{}", root.package, kind_prefix(root.kind));
    let ctx = Ctx {
        file: root.path.clone(),
        is_root: true,
        package: root.package.clone(),
        logical,
        mod_dir: resolution::mod_dir_for(&root.path, true),
        attr_dir: resolution::dir_of(&root.path),
        cfg: None,
    };
    walk_file(fs, tree, &ctx, inv, visited);
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

/// Recursive file walk: extracts the file's items, then follows every
/// `mod x;` (top level or inside an inline module) and literal
/// `include!` edge. `visited` guards against inclusion cycles.
fn walk_file(
    fs: &GitFs,
    tree: &[String],
    ctx: &Ctx,
    inv: &mut Inventory,
    visited: &mut std::collections::BTreeSet<String>,
) {
    if !visited.insert(ctx.file.clone()) {
        return; // cycle guard; identical re-extractions are deduped later
    }
    // Fail closed: a file in the tree listing that cannot be read, or
    // that does not parse, is an INVENTORY_ERROR, not a silent skip.
    let source = match fs.read_checked(&ctx.file) {
        Ok(s) => s,
        Err(e) => {
            inv.errors.push((ctx.file.clone(), format!("{e}")));
            return;
        }
    };
    let parsed = match syn::parse_file(&source) {
        Ok(p) => p,
        Err(e) => {
            inv.errors
                .push((ctx.file.clone(), format!("unparseable Rust source: {e}")));
            return;
        }
    };
    let ext = Extractor {
        package: &ctx.package,
        path: &ctx.file,
        logical_prefix: ctx.logical.clone(),
        inherited_cfg: ctx.cfg.clone(),
    };
    ext.extract(&parsed.items, &mut inv.items);
    process_items(fs, tree, &parsed.items, ctx, inv, visited);
}

/// Follows the module edges of one item list (the items of a file or of
/// an inline module body). Extraction of the items themselves already
/// happened in `walk_file` / the `Extractor`; here only the DECLARATIONS
/// are resolved and recursed into.
fn process_items(
    fs: &GitFs,
    tree: &[String],
    items: &[SynItem],
    ctx: &Ctx,
    inv: &mut Inventory,
    visited: &mut std::collections::BTreeSet<String>,
) {
    for item in items {
        match item {
            SynItem::Mod(m) => {
                let name = m.ident.to_string();
                let cfg = combine_cfg(ctx.cfg.as_deref(), super::item::attr_cfg(&m.attrs));
                match &m.content {
                    // Inline module: no file needed; it extends the
                    // directory context (edition-2018+ rule: `mod a {
                    // mod b; }` resolves `b` under `a/`), its children
                    // may well be EXTERNAL `mod x;` declarations.
                    Some((_, inner)) => {
                        let inner_dir = append_dir(&ctx.mod_dir, &name);
                        let inner_ctx = Ctx {
                            file: ctx.file.clone(),
                            is_root: ctx.is_root,
                            package: ctx.package.clone(),
                            logical: format!("{}::{name}", ctx.logical),
                            mod_dir: inner_dir.clone(),
                            attr_dir: inner_dir,
                            cfg: cfg.clone(),
                        };
                        process_items(fs, tree, inner, &inner_ctx, inv, visited);
                    }
                    None => {
                        // Shared attribute-path extraction (the modules
                        // walker's `path_alternatives`): the previous
                        // local `parse_args::<LitStr>()` could not read a
                        // NAME-VALUE `#[path = "..."]`, so every
                        // path-mounted module silently fell back to
                        // default resolution and its items vanished.
                        let alts = resolution::path_alternatives(&m.attrs);
                        // Handles one resolution outcome; true when a
                        // file was walked. Non-Found resolutions are
                        // INVENTORY_ERROR findings, never silent skips.
                        let handle = |resolved: &Resolution,
                                      inv: &mut Inventory,
                                      visited: &mut std::collections::BTreeSet<String>|
                         -> bool {
                            match resolved {
                                Resolution::Found(child) => {
                                    if !tree.contains(child) {
                                        return false;
                                    }
                                    let child_ctx = Ctx {
                                        file: child.clone(),
                                        is_root: false,
                                        package: ctx.package.clone(),
                                        logical: format!("{}::{name}", ctx.logical),
                                        mod_dir: resolution::mod_dir_for(child, false),
                                        attr_dir: resolution::dir_of(child),
                                        cfg: cfg.clone(),
                                    };
                                    walk_file(fs, tree, &child_ctx, inv, visited);
                                    true
                                }
                                Resolution::Escapes(attr) => {
                                    inv.errors.push((
                                        ctx.file.clone(),
                                        format!(
                                            "mod `{name}` path `{attr}` resolves outside the repository root"
                                        ),
                                    ));
                                    false
                                }
                                _ => false,
                            }
                        };
                        let mut resolved_any = false;
                        if alts.is_empty() {
                            let resolved =
                                resolution::resolve(fs, &name, &ctx.mod_dir, &ctx.attr_dir, None);
                            resolved_any |= handle(&resolved, inv, visited);
                        } else {
                            for alt in alts {
                                let resolved =
                                    resolution::resolve_attr(fs, &ctx.attr_dir, &alt.path);
                                resolved_any |= handle(&resolved, inv, visited);
                            }
                        }
                        if !resolved_any {
                            // Fail closed: an unresolved child module
                            // contributes no items, so its base items
                            // would silently "vanish" — an explicit
                            // INVENTORY_ERROR keeps the comparison honest.
                            inv.errors.push((
                                ctx.file.clone(),
                                format!("child module `{name}` did not resolve at this revision"),
                            ));
                        }
                    }
                }
            }
            SynItem::Macro(im) if macro_name(im) == "include" => {
                if let Ok(lit) = syn::parse2::<syn::LitStr>(im.mac.tokens.clone()) {
                    match resolution::resolve_include(fs, &ctx.file, &lit.value()) {
                        Resolution::Found(target) if target.ends_with(".rs") => {
                            let child_ctx = Ctx {
                                mod_dir: resolution::mod_dir_for(&target, false),
                                attr_dir: resolution::dir_of(&target),
                                file: target,
                                is_root: false,
                                package: ctx.package.clone(),
                                // The included items belong to the SAME
                                // logical module as the includer.
                                logical: ctx.logical.clone(),
                                cfg: ctx.cfg.clone(),
                            };
                            walk_file(fs, tree, &child_ctx, inv, visited);
                        }
                        Resolution::Missing { tried } => inv.errors.push((
                            ctx.file.clone(),
                            format!(
                                "include! target not found at this revision (tried {})",
                                tried.join(", ")
                            ),
                        )),
                        Resolution::Escapes(raw) => inv.errors.push((
                            ctx.file.clone(),
                            format!("include path `{raw}` resolves outside the repository root"),
                        )),
                        // Generated includes are the `modules` gate's
                        // UNRESOLVED_INCLUDE finding; nothing static to
                        // inventory.
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// Appends one segment to a `/`-separated directory context.
fn append_dir(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn macro_name(im: &syn::ItemMacro) -> String {
    im.mac
        .path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default()
}
