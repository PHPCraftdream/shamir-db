//! `mod` declaration resolution rules.
//!
//! Rust's contextual rules, implemented over `/`-separated repo-relative
//! paths. Literal unconditional `#[path = "..."]` resolves relative to the
//! declaring file (or inline module directory); otherwise default Rust
//! module-directory rules apply. Conditional `#[cfg_attr(..., path = "...")]`
//! is intentionally rejected as `UNSUPPORTED_CFG_ATTR_PATH`.

use crate::common::gitfs::GitFs;

/// True when a PARSED cfg predicate is statically unsatisfiable — the
/// forms provably impossible without feature/target evaluation:
/// `any()` (no alternatives), `not(all())`, an `all(...)` containing an
/// unsatisfiable branch, and an `any(...)` whose every branch is
/// unsatisfiable. Works on the syn expression, so token spacing can
/// never influence the outcome. Everything else (feature checks, target
/// comparisons) is treated as satisfiable.
pub fn cfg_unsatisfiable(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Call(call) => {
            let fname = func_name(&call.func);
            let args: Vec<&syn::Expr> = call.args.iter().collect();
            match fname.as_str() {
                "any" => args.is_empty() || args.iter().all(|a| cfg_unsatisfiable(a)),
                "all" => args.iter().any(|a| cfg_unsatisfiable(a)),
                "not" => {
                    args.len() == 1
                        && matches!(args[0], syn::Expr::Call(c) if func_name(&c.func) == "all")
                        && c_is_all_empty(args[0])
                }
                _ => false,
            }
        }
        syn::Expr::Paren(inner) => cfg_unsatisfiable(&inner.expr),
        _ => false,
    }
}

fn c_is_all_empty(expr: &syn::Expr) -> bool {
    matches!(expr, syn::Expr::Call(c) if func_name(&c.func) == "all" && c.args.is_empty())
}

/// Last segment of the callee path (`any`, `all`, `not`, ...).
fn func_name(expr: &syn::Expr) -> String {
    match expr {
        syn::Expr::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Renders a predicate to a canonical token string (for equality tests).
pub fn pred_key(expr: &syn::Expr) -> String {
    quote::ToTokens::to_token_stream(expr).to_string()
}

/// True when two predicates can provably never hold together. Only two
/// structural families are provable without full evaluation:
///
/// * one side is `not(X)` and the other is `X` (modulo
///   parenthesisation/spacing);
/// * both sides are `all(...)` conjunctions (single expressions count as
///   one-element conjunctions) and ANY cross pair of conjuncts is
///   exclusive by the first rule — composed predicates such as
///   `all(unix, feature = "a")` vs `all(not(unix), feature = "b")`
///   therefore compare correctly even though neither top-level form is
///   a bare negation.
pub fn mutually_exclusive(a: &syn::Expr, b: &syn::Expr) -> bool {
    conjuncts(a)
        .iter()
        .any(|x| conjuncts(b).iter().any(|y| exclusive_pair(x, y)))
}

/// Conjuncts of a predicate: the arguments of an `all(...)` call, or the
/// expression itself (a one-element conjunction).
fn conjuncts(expr: &syn::Expr) -> Vec<&syn::Expr> {
    match expr {
        syn::Expr::Call(c) if func_name(&c.func) == "all" => c.args.iter().collect(),
        other => vec![other],
    }
}

/// True when the two single conjuncts are a `not(X)` / `X` pair.
fn exclusive_pair(a: &syn::Expr, b: &syn::Expr) -> bool {
    let (ka, kb) = (pred_key(a), pred_key(b));
    if ka == kb {
        return false;
    }
    negation_of(b, &ka) || negation_of(a, &kb)
}

/// Composes the predicates that guard one reaching edge: the inherited
/// cfg chain and the module's own `#[cfg]`. All present parts are AND-ed
/// (`all(a, b)`); a missing part drops out; an empty composition is `None`
/// (unconditional).
pub fn compose_cfg(parts: &[Option<&syn::Expr>]) -> Option<syn::Expr> {
    let present: Vec<&syn::Expr> = parts.iter().filter_map(|p| *p).collect();
    match present.as_slice() {
        [] => None,
        [one] => Some((*one).clone()),
        [first, rest @ ..] => {
            let mut acc: syn::Expr = syn::parse_quote!(#first);
            for next in rest {
                let next = *next;
                acc = syn::parse_quote!(all(#acc, #next));
            }
            Some(acc)
        }
    }
}

/// Lexically normalises a repo-relative `/` path: `.` segments are
/// dropped and `..` pop the preceding segment. `Err(())` when the path
/// escapes the repo root — the caller must report that, never follow the
/// escape. Normalising before lookup also ensures the visited-key matches
/// the repository-relative file key; otherwise the file can be walked under
/// one spelling and later reported as `UNACCOUNTED_FILE` under another.
pub fn normalize_rel(rel: &str) -> Result<String, ()> {
    let mut out: Vec<&str> = Vec::new();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.pop().is_none() {
                    return Err(());
                }
            }
            other => out.push(other),
        }
    }
    Ok(out.join("/"))
}

/// True when `expr` is `not(inner)` and `inner` renders to `key`.
fn negation_of(expr: &syn::Expr, key: &str) -> bool {
    match expr {
        syn::Expr::Call(c) if func_name(&c.func) == "not" && c.args.len() == 1 => {
            let inner = &c.args[0];
            let inner = match inner {
                syn::Expr::Paren(p) => &p.expr,
                other => other,
            };
            pred_key(inner) == key
        }
        _ => false,
    }
}

/// Outcome of resolving one `mod x;`.
pub enum Resolution {
    /// Found a physical file.
    Found(String),
    /// Both `x.rs` and `x/mod.rs` exist.
    Ambiguous { a: String, b: String },
    /// Neither candidate exists.
    Missing { tried: Vec<String> },
    /// The path attr is not statically resolvable (OUT_DIR, env vars).
    Generated(String),
    /// The resolved path escapes the repository root (`..` beyond the
    /// top): never followed, always reported.
    Escapes(String),
}

/// Directory used for non-`#[path]` resolution of `mod` declarations in
/// `file`. `is_root` marks target-root files (lib.rs, main.rs, tests/*.rs,
/// build.rs, ...), which resolve children in their own directory.
pub fn mod_dir_for(file: &str, is_root: bool) -> String {
    let dir = dir_of(file);
    if is_root || file.ends_with("/mod.rs") || file == "mod.rs" {
        return dir;
    }
    let stem = file
        .rsplit('/')
        .next()
        .and_then(|s| s.strip_suffix(".rs"))
        .unwrap_or("");
    if dir.is_empty() {
        stem.to_owned()
    } else {
        format!("{dir}/{stem}")
    }
}

/// Resolves `mod name;` declared in `file` (with its mod-dir context and
/// optional unconditional `#[path]`). `attr_dir` is the base for `#[path]`
/// resolution: the declaring file's directory, or the inline module's
/// directory context when the `mod` sits inside an inline block.
pub fn resolve(
    fs: &GitFs,
    name: &str,
    mod_dir: &str,
    attr_dir: &str,
    path_attr: Option<&str>,
) -> Resolution {
    if let Some(attr) = path_attr {
        return resolve_attr(fs, attr_dir, attr);
    }
    let base = if mod_dir.is_empty() {
        String::new()
    } else {
        format!("{mod_dir}/")
    };
    let a = format!("{base}{name}.rs");
    let b = format!("{base}{name}/mod.rs");
    let (ea, eb) = (fs.exists(&a), fs.exists(&b));
    match (ea, eb) {
        (true, true) => Resolution::Ambiguous { a, b },
        (true, false) => Resolution::Found(a),
        (false, true) => Resolution::Found(b),
        (false, false) => Resolution::Missing { tried: vec![a, b] },
    }
}

/// Resolves a literal `#[path]` attribute relative to `base_dir` (the
/// declaring file's directory, or the inline module's directory context —
/// rustc rules: inside `mod a { #[path = "b.rs"] mod c; }` in `src/lib.rs`
/// the path resolves at `src/a/b.rs`); appends `.rs` when missing (Rust
/// does the same).
pub fn resolve_attr(fs: &GitFs, base_dir: &str, attr: &str) -> Resolution {
    if attr.contains("OUT_DIR") || attr.contains("env!") || attr.contains("${") {
        return Resolution::Generated(attr.to_owned());
    }
    let rel = if attr.starts_with('/') {
        attr.trim_start_matches('/').to_owned()
    } else if base_dir.is_empty() {
        attr.replace('\\', "/")
    } else {
        format!("{base_dir}/{}", attr.replace('\\', "/"))
    };
    let rel = match normalize_rel(&rel) {
        Ok(r) => r,
        Err(()) => return Resolution::Escapes(attr.to_owned()),
    };
    if fs.exists(&rel) {
        return Resolution::Found(rel);
    }
    let with_ext = format!("{rel}.rs");
    if fs.exists(&with_ext) {
        return Resolution::Found(with_ext);
    }
    Resolution::Missing {
        tried: vec![rel, with_ext],
    }
}

/// Resolves a literal `include!("...")`/`include_str!("...")` relative to
/// the containing file; `Generated` for OUT_DIR / env-var paths.
pub fn resolve_include(fs: &GitFs, file: &str, raw: &str) -> Resolution {
    if raw.contains("OUT_DIR") || raw.contains("env!") || raw.contains("${") || raw.contains('$') {
        return Resolution::Generated(raw.to_owned());
    }
    let dir = dir_of(file);
    let rel = if dir.is_empty() {
        raw.replace('\\', "/")
    } else {
        format!("{dir}/{}", raw.replace('\\', "/"))
    };
    let rel = match normalize_rel(&rel) {
        Ok(r) => r,
        Err(()) => return Resolution::Escapes(raw.to_owned()),
    };
    if fs.exists(&rel) {
        Resolution::Found(rel)
    } else {
        Resolution::Missing { tried: vec![rel] }
    }
}

/// Directory part of a `/`-separated path ("" at the repo root).
pub fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_owned(),
        None => String::new(),
    }
}

// ---------- attribute-path extraction (shared by modules + conservation) ----------

/// Returns the literal unconditional `#[path = "..."]`, if present.
pub fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("path") {
            return None;
        }
        let value = attr.meta.require_name_value().ok()?;
        let syn::Expr::Lit(lit) = &value.value else {
            return None;
        };
        let syn::Lit::Str(path) = &lit.lit else {
            return None;
        };
        Some(path.value())
    })
}

/// Detects direct `path` metadata at any nesting level under a `cfg_attr`.
/// Invalid cfg_attr syntax is handled as a parse/source error elsewhere.
pub fn cfg_attr_has_path(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg_attr")
            && attr
                .parse_args::<CfgAttrArgs>()
                .is_ok_and(|args| args.attrs.iter().any(meta_has_path))
    })
}

fn meta_has_path(meta: &syn::Meta) -> bool {
    if meta.path().is_ident("path") {
        return true;
    }
    meta.path().is_ident("cfg_attr")
        && meta
            .require_list()
            .ok()
            .and_then(|list| syn::parse2::<CfgAttrArgs>(list.tokens.clone()).ok())
            .is_some_and(|args| args.attrs.iter().any(meta_has_path))
}

/// Parsed contents of `#[cfg_attr(pred, attr, attr, ...)]`.
struct CfgAttrArgs {
    _pred: syn::Meta,
    attrs: syn::punctuated::Punctuated<syn::Meta, syn::Token![,]>,
}

impl syn::parse::Parse for CfgAttrArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let pred: syn::Meta = input.call(syn::Meta::parse)?;
        input.parse::<syn::Token![,]>()?;
        let attrs =
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated(input)?;
        Ok(Self { _pred: pred, attrs })
    }
}
