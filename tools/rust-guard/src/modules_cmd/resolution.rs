//! `mod` declaration resolution rules.
//!
//! Rust's contextual rules, implemented over `/`-separated repo-relative
//! paths:
//!
//! * `#[path = "..."]` (literal, or via `#[cfg_attr(pred, path = "...")]`)
//!   is relative to the directory of the file containing the `mod`.
//! * Otherwise, for `mod x;` in file `F`: if `F` is a target root or named
//!   `mod.rs`, candidates are `dir(F)/x.rs` and `dir(F)/x/mod.rs`; for any
//!   other file `dir(F)/stem.rs`, candidates live under `dir(F)/stem/`.
//! * Both candidates existing is an ambiguity error.

use crate::gitfs::GitFs;

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
/// optional `#[path]`/`cfg_attr` alternative).
pub fn resolve(
    fs: &GitFs,
    file: &str,
    name: &str,
    mod_dir: &str,
    path_attr: Option<&str>,
) -> Resolution {
    let file_dir = dir_of(file);
    if let Some(attr) = path_attr {
        return resolve_attr(fs, &file_dir, attr);
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

/// Resolves a literal `#[path]` attribute relative to the declaring file's
/// directory; appends `.rs` when missing (Rust does the same).
fn resolve_attr(fs: &GitFs, file_dir: &str, attr: &str) -> Resolution {
    if attr.contains("OUT_DIR") || attr.contains("env!") || attr.contains("${") {
        return Resolution::Generated(attr.to_owned());
    }
    let rel = if attr.starts_with('/') {
        attr.trim_start_matches('/').to_owned()
    } else if file_dir.is_empty() {
        attr.replace('\\', "/")
    } else {
        format!("{file_dir}/{}", attr.replace('\\', "/"))
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

/// True when the predicate text is statically unsatisfiable (`any()` with
/// no arguments — the only form provably impossible without evaluation).
pub fn cfg_unsatisfiable(pred: &str) -> bool {
    let t = pred.trim();
    t == "any()" || t == "any( )"
}
