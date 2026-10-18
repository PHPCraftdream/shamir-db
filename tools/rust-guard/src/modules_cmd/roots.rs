//! Discovery of package/target roots.
//!
//! Sources: `cargo metadata --no-deps` workspace members (with explicit
//! filtering of vendored packages) plus hand-parsed manifests of
//! workspace-excluded packages (`crates/shamir-client-node`, `tools/*`).
//! Manifest target sections and auto-discovery flags are honored; files
//! under `tests/` beyond the cargo roots (`tests/*.rs`, `tests/*/main.rs`)
//! are never roots.

use super::json;
use crate::gitfs::GitFs;

/// Kind of a target root file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    Lib,
    Bin,
    Test,
    Bench,
    Example,
    Build,
}

/// One file that starts a module tree.
#[derive(Debug, Clone)]
pub struct TargetRoot {
    pub path: String,
    pub kind: RootKind,
    pub package: String,
}

/// Discovers all target roots from the given filesystem view.
pub fn discover(fs: &GitFs) -> Result<Vec<TargetRoot>, String> {
    let mut roots = Vec::new();
    for manifest in manifest_paths(fs)? {
        add_targets(fs, &manifest, &mut roots);
    }
    Ok(roots)
}

/// Every package manifest to consider: workspace members from cargo
/// metadata plus workspace-excluded packages. `vendor/*` packages are
/// explicitly dropped from the scan scope.
fn manifest_paths(fs: &GitFs) -> Result<Vec<String>, String> {
    let mut manifests: Vec<String> = Vec::new();
    let mut push = |m: String| {
        if !m.starts_with("vendor/") && !manifests.contains(&m) {
            manifests.push(m);
        }
    };

    let meta = run_cargo_metadata(fs)?;
    let members = meta.get("members").map(|m| m.str_set()).unwrap_or_default();
    if let Some(packages) = meta.get("packages").and_then(|p| p.as_arr()) {
        for pkg in packages {
            let id = pkg.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let path = pkg
                .get("manifest_path")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            if members.contains(id) && !path.is_empty() {
                push(rel_from_abs(fs, path));
            }
        }
    }

    // Workspace-excluded packages that still ship Rust sources.
    for candidate in ["crates/shamir-client-node/Cargo.toml"] {
        if fs.exists(candidate) {
            push(candidate.to_owned());
        }
    }
    for f in fs.tracked_files()? {
        if f.starts_with("tools/") && f.ends_with("/Cargo.toml") && f != "tools/Cargo.toml" {
            push(f);
        }
    }
    Ok(manifests)
}

/// Runs `cargo metadata --no-deps` from the repo root and parses it.
fn run_cargo_metadata(fs: &GitFs) -> Result<json::Json, String> {
    let out = std::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(fs.root())
        .output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    json::parse(&String::from_utf8_lossy(&out.stdout))
}

/// Converts an absolute manifest path to a repo-relative `/` path.
fn rel_from_abs(fs: &GitFs, abs: &str) -> String {
    let root = fs.root().to_string_lossy().replace('\\', "/");
    let norm = abs.replace('\\', "/");
    norm.strip_prefix(&root)
        .unwrap_or(&norm)
        .trim_start_matches('/')
        .to_owned()
}

/// Parses one manifest and appends its target roots.
fn add_targets(fs: &GitFs, manifest: &str, roots: &mut Vec<TargetRoot>) {
    let Some(text) = fs.read(manifest) else {
        return;
    };
    let Ok(doc) = text.parse::<toml::Table>() else {
        return;
    };
    let pkg = doc.get("package").and_then(|p| p.as_table());
    let Some(pkg) = pkg else { return }; // virtual manifest: no targets
    let package = pkg
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("unknown")
        .to_owned();
    let dir = parent_dir(manifest);

    let auto = |flag: &str| -> bool { pkg.get(flag).and_then(|v| v.as_bool()).unwrap_or(true) };
    let exists = |p: &str| fs.exists(p);

    // [lib] — explicit path or the default location.
    let lib = doc.get("lib").and_then(|t| t.as_table());
    let lib_path = lib
        .and_then(|t| t.get("path"))
        .and_then(|p| p.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{dir}/src/lib.rs"));
    if exists(&lib_path) {
        roots.push(TargetRoot {
            path: lib_path,
            kind: RootKind::Lib,
            package: package.clone(),
        });
    }

    // build.rs — [package] build key or the conventional file.
    let build_path = pkg
        .get("build")
        .and_then(|b| b.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{dir}/build.rs"));
    if exists(&build_path) {
        roots.push(TargetRoot {
            path: build_path,
            kind: RootKind::Build,
            package: package.clone(),
        });
    }

    // [[bin]] / [[test]] / [[bench]] / [[example]]: explicit entries first,
    // then auto-discovery unless the matching auto flag is false.
    let targets = [
        ("bin", RootKind::Bin, "src/bin", auto("autobins")),
        ("test", RootKind::Test, "tests", auto("autotests")),
        ("bench", RootKind::Bench, "benches", auto("autobenches")),
        (
            "example",
            RootKind::Example,
            "examples",
            auto("autoexamples"),
        ),
    ];
    for (key, kind, conv_dir, auto_flag) in targets {
        let spec = TargetSpec {
            kind,
            conv_dir,
            auto: auto_flag,
        };
        add_explicit(fs, doc.get(key), &dir, &package, &spec, roots);
    }

    // src/main.rs is a bin root only when not already declared explicitly
    // (an explicit `[[bin]] path` replaces the default discovery).
    let default_main = format!("{dir}/src/main.rs");
    if auto("autobins") && exists(&default_main) {
        let declared = doc
            .get("bin")
            .and_then(|b| b.as_array())
            .map(|a| {
                a.iter()
                    .any(|t| t.get("path").and_then(|p| p.as_str()) == Some("src/main.rs"))
            })
            .unwrap_or(false);
        if !declared {
            roots.push(TargetRoot {
                path: default_main,
                kind: RootKind::Bin,
                package: package.clone(),
            });
        }
    }
}

/// Auto-discovery configuration for one target kind.
struct TargetSpec {
    kind: RootKind,
    conv_dir: &'static str,
    auto: bool,
}

/// Adds targets from one explicit table array, falling back to auto-
/// discovery of the conventional directory when the array is absent.
fn add_explicit(
    fs: &GitFs,
    entries: Option<&toml::Value>,
    dir: &str,
    package: &str,
    spec: &TargetSpec,
    roots: &mut Vec<TargetRoot>,
) {
    let kind = spec.kind;
    let exists = |p: &str| fs.exists(p);
    let mut seen: Vec<String> = Vec::new();
    if let Some(arr) = entries.and_then(|e| e.as_array()) {
        for t in arr {
            let Some(path) = t.get("path").and_then(|p| p.as_str()) else {
                continue;
            };
            let path = join_rel(dir, path);
            if exists(&path) && !seen.contains(&path) {
                seen.push(path.clone());
                roots.push(TargetRoot {
                    path,
                    kind,
                    package: package.to_owned(),
                });
            }
        }
    }
    if !spec.auto {
        return;
    }
    // Conventional auto-discovery: files directly in the dir, plus
    // `<dir>/<name>/main.rs` subdirectory targets.
    let prefix = format!("{dir}/{}/", spec.conv_dir);
    for f in fs.tracked_files().unwrap_or_default() {
        if !f.starts_with(&prefix) || !f.ends_with(".rs") || seen.contains(&f) {
            continue;
        }
        let rest = &f[prefix.len()..];
        let is_root = !rest.contains('/') || rest.ends_with("/main.rs");
        if is_root && !seen.contains(&f.to_owned()) {
            seen.push(f.clone());
            roots.push(TargetRoot {
                path: f,
                kind,
                package: package.to_owned(),
            });
        }
    }
}

/// Joins a manifest-relative target path onto the manifest directory.
fn join_rel(dir: &str, path: &str) -> String {
    let path = path.replace('\\', "/");
    if let Some(rest) = path.strip_prefix("./") {
        return format!("{dir}/{rest}");
    }
    if path.starts_with('/') {
        return path;
    }
    format!("{dir}/{path}")
}

/// Parent directory of a `/`-separated path ("" for top-level files).
fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_owned(),
        None => String::new(),
    }
}
