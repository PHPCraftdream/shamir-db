//! Rule 5 — `OWNED_IMPL`: an inherent `impl` of a configured type must
//! live in the type's defining module or a descendant of it.
//!
//! Configuration in `.rust-guard/owned_impl.toml`:
//!
//! ```toml
//! [[owned_impl]]
//! type = "RepoInstance"
//! module = "crate::repo::instance::repo_instance"
//! ```

use crate::finding::Report;
use serde::Deserialize;
use syn::{File, Item, Type};

pub(crate) const OWNED_IMPL_FILE: &str = ".rust-guard/owned_impl.toml";

/// One configured owned-impl assertion.
#[derive(Debug, Deserialize)]
pub struct OwnedImpl {
    /// Self-type name of inherent impls this entry governs.
    pub r#type: String,
    /// Logical module path (crate-rooted) that owns the type.
    pub module: String,
}

/// Parsed `.rust-guard/owned_impl.toml`.
#[derive(Debug, Deserialize, Default)]
pub struct OwnedImplConfig {
    #[serde(default, rename = "owned_impl")]
    pub owned_impl: Vec<OwnedImpl>,
}

impl OwnedImplConfig {
    /// Loads the config; a missing file is an empty set (no assertion).
    pub fn load(path: &str) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("cannot read {path}: {e}")),
        };
        toml::from_str(&text).map_err(|e| format!("cannot parse {path}: {e}"))
    }

    /// The entry governing `type_name`, if any.
    fn for_type(&self, type_name: &str) -> Option<&OwnedImpl> {
        self.owned_impl.iter().find(|e| e.r#type == type_name)
    }
}

/// Rule 5 over one file: every inherent `impl` whose self type is
/// configured must sit in the defining module or its descendants.
pub fn check_file(path: &str, file: &File, cfg: &OwnedImplConfig, report: &mut Report) {
    let Some(logical) = logical_module_path(path) else {
        return;
    };
    for item in &file.items {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() {
            continue; // trait impls are not governed
        }
        let Some(name) = self_type_name(&imp.self_ty) else {
            continue;
        };
        let Some(entry) = cfg.for_type(&name) else {
            continue;
        };
        let owned = logical == entry.module || logical.starts_with(&format!("{}::", entry.module));
        if !owned {
            report.add(
                path,
                imp.impl_token.span.start().line,
                "OWNED_IMPL",
                format!(
                    "inherent impl of `{name}` must live in `{}` or its descendants (here: `{logical}`)",
                    entry.module
                ),
            );
        }
    }
}

/// First path segment of the impl self type (`RepoInstance` from
/// `RepoInstance<..>` or qualified paths); `None` for other shapes.
fn self_type_name(ty: &Type) -> Option<String> {
    if let Type::Path(p) = ty {
        return p.path.segments.first().map(|s| s.ident.to_string());
    }
    None
}

/// Logical (crate-rooted) module path of a source file, derived from the
/// physical path: everything after the package's `src/` directory, with
/// `mod.rs` collapsing into its directory and `x.rs` contributing `x`.
/// `src/lib.rs` and `src/main.rs` map to `crate`. Heuristic: exact for
/// standard layouts; files with `#[path]`-mounted members may diverge.
fn logical_module_path(path: &str) -> Option<String> {
    let idx = path.find("/src/")?;
    let rest = &path[idx + "/src/".len()..];
    let mut segs: Vec<&str> = rest.split('/').collect();
    match segs.last().copied() {
        Some("lib.rs") | Some("main.rs") => {
            segs.pop();
        }
        Some("mod.rs") => {
            segs.pop();
        }
        Some(f) if f.ends_with(".rs") => {
            let n = segs.len();
            segs[n - 1] = f.strip_suffix(".rs")?;
        }
        _ => return None,
    }
    let mut out = String::from("crate");
    for s in segs {
        out.push_str("::");
        out.push_str(s);
    }
    Some(out)
}
