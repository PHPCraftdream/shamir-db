//! Module map of one crate: files, logical module paths, module-name
//! bindings (`use` re-exports) and type ownership, all under the
//! crate's `src/` tree. Test code (`#[cfg(test)]`/loom items and
//! test-named modules) is not part of the map. Shadow rule: a re-export
//! of an EXTERNAL item is a shadow, not an edge — references through it
//! never attribute to the declaring module.

use crate::commands::modules_cmd::resolution;
use crate::common::gitfs::GitFs;
use std::collections::{BTreeMap, BTreeSet};

/// Logical module path (segment vector; crate root = empty).
pub(crate) type ModPath = Vec<String>;

/// Per-crate module map.
pub(crate) struct CrateGraph {
    /// Repo-relative src directory, e.g. `crates/engine/shamir-engine/src`.
    /// Diagnostic metadata for callers; not consulted by the map itself.
    #[expect(dead_code)] // required by the layers_cmd contract; diagnostic metadata
    pub(crate) src_prefix: String,
    /// Repo-relative file -> logical module path of the file.
    pub(crate) files: BTreeMap<String, ModPath>,
    /// Every module path seen (file-backed AND inline).
    pub(crate) known: BTreeSet<ModPath>,
    /// (declaring module, local name) -> (base module, remaining
    /// segments) of a `use` item, in absolute base form.
    pub(crate) bindings: BTreeMap<(ModPath, String), (ModPath, Vec<String>)>,
    /// Owner module of every binding, filled by `canonicalize`. `None`
    /// means the binding resolves OUTSIDE this crate (an external
    /// shadow); `Some(path)` is the local owner module.
    pub(crate) canonical: BTreeMap<(ModPath, String), Option<ModPath>>,
    /// Modules with a `use ...::*` whose source resolves to a LOCAL
    /// module path.
    pub(crate) globs_local: BTreeSet<ModPath>,
    /// Modules with a glob whose source resolves to nothing local
    /// (first segment unknown etc.).
    pub(crate) globs_external: BTreeSet<ModPath>,
    /// Logical module path -> owning file (first mount wins). The crate
    /// root (`[]`) maps to its root file.
    pub(crate) mod_files: BTreeMap<ModPath, String>,
    /// struct/enum/union/trait name -> declaring modules.
    pub(crate) types: BTreeMap<String, BTreeSet<ModPath>>,
}

/// True for test-only module names: `tests`, `*_tests`, `*_loom`.
pub(crate) fn is_test_name(name: &str) -> bool {
    name == "tests" || name.ends_with("_tests") || name.ends_with("_loom")
}

/// True when the attributes mark the item as test code: any `cfg`
/// predicate mentioning `test`/`loom`, or a bare `#[test]`/`#[tokio::test]`.
/// Unparseable predicates are NOT test (conservative: the item stays in
/// the map).
pub(crate) fn cfg_mentions_test(attrs: &[syn::Attribute]) -> bool {
    for attr in attrs {
        let path = attr.path();
        if path.is_ident("test") || path.is_ident("tokio::test") {
            return true;
        }
        if !path.is_ident("cfg") {
            continue;
        }
        if let Ok(expr) = attr.parse_args::<syn::Expr>() {
            if expr_mentions_test(&expr) {
                return true;
            }
        }
    }
    false
}

/// Recurses into the expression forms a cfg predicate uses, looking for
/// the idents `test` or `loom`.
fn expr_mentions_test(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Path(p) => p
            .path
            .segments
            .iter()
            .any(|s| s.ident == "test" || s.ident == "loom"),
        syn::Expr::Call(c) => expr_mentions_test(&c.func) || c.args.iter().any(expr_mentions_test),
        syn::Expr::Paren(inner) => expr_mentions_test(&inner.expr),
        syn::Expr::Assign(a) => expr_mentions_test(&a.left) || expr_mentions_test(&a.right),
        syn::Expr::Tuple(t) => t.elems.iter().any(expr_mentions_test),
        _ => false,
    }
}

/// One imported name from a use tree: remaining path segments and the
/// local name (empty for a glob).
pub(crate) struct UseLeaf {
    pub(crate) segs: Vec<String>,
    pub(crate) local: String,
}

/// Flattens a use tree into its imported names.
pub(crate) fn expand_use_tree(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    out: &mut Vec<UseLeaf>,
) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            expand_use_tree(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let ident = n.ident.to_string();
            prefix.push(ident.clone());
            out.push(UseLeaf {
                segs: prefix.clone(),
                local: ident,
            });
            prefix.pop();
        }
        syn::UseTree::Rename(r) => {
            prefix.push(r.ident.to_string());
            out.push(UseLeaf {
                segs: prefix.clone(),
                local: r.rename.to_string(),
            });
            prefix.pop();
        }
        syn::UseTree::Glob(_) => {
            // The glob itself is a reference; no trailing name, empty
            // local.
            out.push(UseLeaf {
                segs: prefix.clone(),
                local: String::new(),
            });
        }
        syn::UseTree::Group(g) => {
            for item in &g.items {
                expand_use_tree(item, prefix, out);
            }
        }
    }
}

/// Normalizes the FIRST segment of a use path to an absolute base:
/// `crate` -> root, `self` -> the declaring module, k `super`s truncate
/// the module, anything else stays relative (external crate or
/// intra-module item).
pub(crate) fn absolute_base(logical: &ModPath, segs: &[String]) -> (ModPath, Vec<String>) {
    let first = segs.first().map(String::as_str);
    match first {
        Some("crate") => (Vec::new(), segs[1..].to_vec()),
        Some("self") => (logical.clone(), segs[1..].to_vec()),
        Some("super") => {
            let mut k = 0;
            while k < segs.len() && segs[k] == "super" {
                k += 1;
            }
            let cut = logical.len().saturating_sub(k);
            (logical[..cut].to_vec(), segs[k..].to_vec())
        }
        _ => (logical.clone(), segs.to_vec()),
    }
}

#[derive(serde::Deserialize)]
struct Manifest {
    #[serde(default)]
    lib: Option<LibTarget>,
    #[serde(default)]
    package: Option<Package>,
}

#[derive(serde::Deserialize)]
struct Package {
    name: String,
}

#[derive(serde::Deserialize)]
struct LibTarget {
    #[serde(default)]
    path: Option<String>,
}

/// Builds the module map for one crate by walking its target roots.
pub(crate) fn build(
    fs: &GitFs,
    _repo: &std::path::Path,
    crate_name: &str,
) -> Result<CrateGraph, String> {
    let tracked = fs.tracked_files()?;
    let mut src_prefix = None;
    let mut lib_path = None;
    for file in &tracked {
        let parts: Vec<&str> = file.split('/').collect();
        if parts.len() != 4 || parts[0] != "crates" || parts[3] != "Cargo.toml" {
            continue;
        }
        let manifest = fs
            .read(file)
            .ok_or_else(|| format!("{file}: unreadable manifest"))?;
        let parsed: Manifest = toml::from_str(&manifest).map_err(|e| format!("{file}: {e}"))?;
        let name = parsed.package.map(|p| p.name).unwrap_or_default();
        if name != crate_name {
            continue;
        }
        let dir = resolution::dir_of(file);
        lib_path = parsed.lib.and_then(|l| l.path).map(|p| joined(&dir, &p));
        src_prefix = Some(format!("{dir}/src"));
    }
    let src_prefix =
        src_prefix.ok_or_else(|| format!("crate `{crate_name}` not found under crates/*/"))?;
    let mut roots = Vec::new();
    roots.push(lib_path.unwrap_or_else(|| joined(&src_prefix, "lib.rs")));
    roots.push(joined(&src_prefix, "main.rs"));
    let bin_prefix = format!("{src_prefix}/bin/");
    for file in &tracked {
        if let Some(rest) = file.strip_prefix(&bin_prefix) {
            if rest == "main.rs" || (rest.ends_with(".rs") && !rest.contains('/')) {
                roots.push(file.clone());
            }
        }
    }
    roots.retain(|r| fs.exists(r));
    if roots.is_empty() {
        return Err(format!(
            "no lib/main target root found for crate `{crate_name}`"
        ));
    }
    let mut graph = CrateGraph {
        src_prefix,
        files: BTreeMap::new(),
        known: BTreeSet::new(),
        bindings: BTreeMap::new(),
        canonical: BTreeMap::new(),
        globs_local: BTreeSet::new(),
        globs_external: BTreeSet::new(),
        mod_files: BTreeMap::new(),
        types: BTreeMap::new(),
    };
    for root in &roots {
        let dir = resolution::dir_of(root);
        walk_file(
            fs,
            root,
            Vec::new(),
            &dir,
            &dir,
            &mut Vec::new(),
            &mut graph,
        )?;
    }
    graph.canonicalize();
    for (file, logical) in graph.files.clone() {
        graph.mod_files.entry(logical).or_insert(file);
    }
    for root in &roots {
        graph
            .mod_files
            .entry(Vec::new())
            .or_insert_with(|| root.clone());
    }
    Ok(graph)
}

/// Joins a directory and a relative path with `/`.
fn joined(dir: &str, rel: &str) -> String {
    if dir.is_empty() {
        rel.to_owned()
    } else {
        format!("{dir}/{rel}")
    }
}

/// Walks one physical file, mounting it at `logical` and resolving its
/// child modules. `mod_dir`/`attr_dir` follow the resolution rules of the
/// modules gate; `ancestors` guards against mount cycles.
fn walk_file(
    fs: &GitFs,
    file: &str,
    logical: ModPath,
    mod_dir: &str,
    attr_dir: &str,
    ancestors: &mut Vec<String>,
    graph: &mut CrateGraph,
) -> Result<(), String> {
    let source = fs.read(file).ok_or_else(|| format!("{file}: unreadable"))?;
    let ast = syn::parse_file(&source).map_err(|e| format!("{file}: {e}"))?;
    graph
        .files
        .entry(file.to_owned())
        .or_insert_with(|| logical.clone());
    graph.known.insert(logical.clone());
    ancestors.push(file.to_owned());
    for item in &ast.items {
        process_item(fs, item, &logical, mod_dir, attr_dir, ancestors, graph)?;
    }
    ancestors.pop();
    Ok(())
}

/// Attributes of any item kind (syn has no shared accessor).
pub(crate) fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::ForeignMod(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// Processes one item of a walked file (or of an inline module block).
fn process_item(
    fs: &GitFs,
    item: &syn::Item,
    logical: &ModPath,
    mod_dir: &str,
    attr_dir: &str,
    ancestors: &mut Vec<String>,
    graph: &mut CrateGraph,
) -> Result<(), String> {
    let attrs = item_attrs(item);
    if cfg_mentions_test(attrs) {
        return Ok(());
    }
    match item {
        syn::Item::Mod(m) => handle_mod(fs, m, logical, mod_dir, attr_dir, ancestors, graph),
        syn::Item::Use(u) => {
            let mut prefix = Vec::new();
            let mut leaves = Vec::new();
            expand_use_tree(&u.tree, &mut prefix, &mut leaves);
            for leaf in leaves {
                let base = absolute_base(logical, &leaf.segs);
                if !leaf.local.is_empty() {
                    graph.bindings.insert((logical.clone(), leaf.local), base);
                } else {
                    // A glob: record whether its source is local or
                    // external. KNOWN-ONLY walk (canonical is empty
                    // during the build).
                    if known_walk(&graph.known, &base.0, &base.1).is_empty() {
                        graph.globs_external.insert(logical.clone());
                    } else {
                        graph.globs_local.insert(logical.clone());
                    }
                }
            }
            Ok(())
        }
        syn::Item::Struct(i) => {
            record_type(graph, &i.ident.to_string(), logical);
            Ok(())
        }
        syn::Item::Enum(i) => {
            record_type(graph, &i.ident.to_string(), logical);
            Ok(())
        }
        syn::Item::Union(i) => {
            record_type(graph, &i.ident.to_string(), logical);
            Ok(())
        }
        syn::Item::Trait(i) => {
            record_type(graph, &i.ident.to_string(), logical);
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Registers a type name in the map.
fn record_type(graph: &mut CrateGraph, name: &str, logical: &ModPath) {
    graph
        .types
        .entry(name.to_owned())
        .or_default()
        .insert(logical.clone());
}

/// KNOWN-ONLY walk of `segs` from `base`: descends through `known`
/// modules only; empty result = no local module path.
fn known_walk(known: &BTreeSet<ModPath>, base: &ModPath, segs: &[String]) -> ModPath {
    let mut cur = base.clone();
    for s in segs {
        let mut child = cur.clone();
        child.push(s.clone());
        if !known.contains(&child) {
            return Vec::new();
        }
        cur = child;
    }
    cur
}

/// Resolves and descends into a `mod` declaration.
fn handle_mod(
    fs: &GitFs,
    m: &syn::ItemMod,
    logical: &ModPath,
    mod_dir: &str,
    attr_dir: &str,
    ancestors: &mut Vec<String>,
    graph: &mut CrateGraph,
) -> Result<(), String> {
    let name = m.ident.to_string();
    if is_test_name(&name) || cfg_mentions_test(&m.attrs) {
        return Ok(());
    }
    if resolution::cfg_attr_has_path(&m.attrs) {
        return Err(format!("mod `{name}`: unsupported cfg_attr(path) syntax"));
    }
    let child_logical = [logical.as_slice(), std::slice::from_ref(&name)].concat();
    let child_dir = joined(mod_dir, &name);
    match &m.content {
        Some((_, items)) => {
            for item in items {
                process_item(
                    fs,
                    item,
                    &child_logical,
                    &child_dir,
                    &child_dir,
                    ancestors,
                    graph,
                )?;
            }
            Ok(())
        }
        None => {
            let path = resolution::path_attr(&m.attrs);
            match resolution::resolve(fs, &name, mod_dir, attr_dir, path.as_deref()) {
                resolution::Resolution::Found(child) => {
                    if ancestors.contains(&child) {
                        return Err(format!("mod `{name}` reaches ancestor file `{child}`"));
                    }
                    let dir = resolution::dir_of(&child);
                    walk_file(
                        fs,
                        &child,
                        child_logical,
                        &resolution::mod_dir_for(&child, false),
                        &dir,
                        ancestors,
                        graph,
                    )
                }
                resolution::Resolution::Missing { tried } => Err(format!(
                    "mod `{name}`: missing module file (tried {})",
                    tried.join(", ")
                )),
                resolution::Resolution::Ambiguous { a, b } => {
                    Err(format!("mod `{name}`: ambiguous module file ({a} vs {b})"))
                }
                resolution::Resolution::Escapes(p) => {
                    Err(format!("mod `{name}`: path `{p}` escapes the repo"))
                }
                resolution::Resolution::Generated(p) => {
                    Err(format!("mod `{name}`: generated path `{p}` unsupported"))
                }
            }
        }
    }
}

impl CrateGraph {
    /// Resolves every binding to its owner module (or to `None` for an
    /// EXTERNAL shadow) to a fixpoint: each pass reads the (partially
    /// filled) canonical map and only ever produces MORE resolved
    /// entries — `None` is a value, never an unresolved placeholder.
    pub(crate) fn canonicalize(&mut self) {
        for _ in 0..16 {
            let mut changed = false;
            for ((m, n), (base, segs)) in &self.bindings {
                let found = self.resolve_from(base, segs);
                let target = if found.is_empty() { None } else { Some(found) };
                if self.canonical.get(&(m.clone(), n.clone())) != Some(&target) {
                    self.canonical.insert((m.clone(), n.clone()), target);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Steps a use path from `base` through known modules, canonical
    /// bindings and local surface. An EMPTY result means "no edge / not
    /// in this crate" — callers drop it. Shadow rule: a re-export of an
    /// external item is a shadow, not an edge (`Some(None)` and
    /// external globs both stop attribution).
    pub(crate) fn resolve_from(&self, base: &ModPath, segs: &[String]) -> ModPath {
        if segs.is_empty() {
            return base.clone();
        }
        let mut cur = base.clone();
        let mut jumps = 0;
        for (i, s) in segs.iter().enumerate() {
            // 1. Canonical binding.
            match self.canonical.get(&(cur.clone(), s.clone())) {
                Some(Some(t)) => {
                    cur = t.clone();
                    jumps += 1;
                    if jumps > 16 {
                        return Vec::new();
                    }
                    continue;
                }
                // External shadow: never attribute to the declaring
                // module.
                Some(None) => return Vec::new(),
                // Binding not yet canonicalized (mid-fixpoint only).
                None => {}
            }
            // 2. Known submodule.
            let mut child = cur.clone();
            child.push(s.clone());
            if self.known.contains(&child) {
                cur = child;
                continue;
            }
            // 3. External crate or bogus first segment.
            if i == 0 {
                return Vec::new();
            }
            // 4. Locally defined type of THIS module.
            if self.types.get(s).is_some_and(|m| m.contains(&cur)) {
                break;
            }
            // 5. A local glob may provide the name.
            if self.globs_local.contains(&cur) {
                break;
            }
            // 6. External glob shadow.
            if self.globs_external.contains(&cur) {
                return Vec::new();
            }
            // 7. File modules and INLINE modules expose their items
            // directly (an inline module has no file of its own).
            let is_inline = !self.mod_files.contains_key(&cur);
            let is_file_mount = self
                .mod_files
                .get(&cur)
                .is_some_and(|f| !f.ends_with("mod.rs"));
            if !cur.is_empty() && (is_inline || is_file_mount) {
                break;
            }
            // 8. A mod.rs directory module (or the crate root) whose
            // surface neither binds nor globs the name.
            return Vec::new();
        }
        cur
    }

    /// Sorted declaring modules of a type name (empty when unknown).
    pub(crate) fn owner_modules(&self, type_name: &str) -> Vec<ModPath> {
        self.types
            .get(type_name)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }
}
