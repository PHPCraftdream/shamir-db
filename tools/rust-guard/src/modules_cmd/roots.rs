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

/// A manifest-level problem discovered while scanning roots. Callers
/// turn these into `BAD_MANIFEST` findings (fail closed): a manifest
/// that is unreadable, unparseable, or names undeclared
/// `required-features` must never be silently skipped, or every target
/// under it disappears from the scan.
#[derive(Debug, Clone)]
pub struct RootProblem {
    pub manifest: String,
    pub message: String,
}

/// Discovers all target roots plus the manifest problems encountered.
pub fn discover_with_problems(fs: &GitFs) -> Result<(Vec<TargetRoot>, Vec<RootProblem>), String> {
    let mut roots = Vec::new();
    let mut problems = Vec::new();
    for manifest in manifest_paths(fs)? {
        add_targets(fs, &manifest, &mut roots, &mut problems);
    }
    Ok((roots, problems))
}

/// Discovers target roots purely from git objects — NO cargo. Parses the
/// ROOT `Cargo.toml` on `fs` (`[workspace]` members/exclude, `*` globs
/// expanded segment-wise against the pinned tree listing) plus every
/// member manifest on the same pinned view. Used by the `conservation`
/// gate so BOTH revision inventories come from their own revision: the
/// working tree's `cargo metadata` would silently drop targets that the
/// BASE still had (finding 7). Problems fail closed like everywhere else.
pub fn discover_from_manifests(fs: &GitFs) -> Result<(Vec<TargetRoot>, Vec<RootProblem>), String> {
    let mut roots = Vec::new();
    let mut problems = Vec::new();
    let listing = pinned_files(fs)?;
    let manifest = "Cargo.toml";
    let mut problem = |message: String| {
        problems.push(RootProblem {
            manifest: manifest.to_owned(),
            message,
        });
    };
    let text = match fs.read_checked(manifest) {
        Ok(t) => t,
        Err(e) => {
            problem(format!("root manifest: {e}"));
            return Ok((roots, problems));
        }
    };
    let Ok(doc) = text.parse::<toml::Table>() else {
        problem("root manifest is not parseable TOML".into());
        return Ok((roots, problems));
    };
    if let Some(ws) = doc.get("workspace").and_then(|w| w.as_table()) {
        let members: Vec<String> = ws
            .get("members")
            .and_then(|m| m.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if members.is_empty() {
            problem("workspace declares no members".into());
        }
        let exclude: Vec<String> = ws
            .get("exclude")
            .and_then(|e| e.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        for pat in &members {
            for dir in expand_member_glob(pat, &listing) {
                if dir.starts_with("vendor/") || exclude.iter().any(|e| glob_matches(e, &dir)) {
                    continue;
                }
                add_targets(fs, &format!("{dir}/Cargo.toml"), &mut roots, &mut problems);
            }
        }
    } else if doc.get("package").is_some() {
        // Single-package repository: the root manifest is the package.
        add_targets(fs, manifest, &mut roots, &mut problems);
    } else {
        problem("root manifest has neither [workspace] nor [package]".into());
    }
    Ok((roots, problems))
}

/// File listing of the pinned view (`ls-tree` at the rev); falls back to
/// the tracked-file listing when no revision is pinned.
fn pinned_files(fs: &GitFs) -> Result<Vec<String>, String> {
    match fs.list_tree() {
        Ok(t) => Ok(t),
        Err(e) if e.contains("no revision pinned") => fs.tracked_files(),
        Err(e) => Err(e),
    }
}

/// Every directory of the listing matching a workspace-members pattern.
/// Patterns are `/`-separated; `*` matches exactly one path segment.
fn expand_member_glob(pattern: &str, listing: &[String]) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    for f in listing {
        let segs: Vec<&str> = f.split('/').collect();
        let mut acc = String::new();
        // All ancestor directories of the file (file name dropped).
        for seg in &segs[..segs.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            if !dirs.contains(&acc.clone()) {
                dirs.push(acc.clone());
            }
        }
    }
    dirs.retain(|d| glob_matches(pattern, d));
    dirs
}

/// Segment-wise glob match: `*` matches exactly one `/`-separated
/// segment, everything else must be equal. Empty patterns match nothing.
fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let c: Vec<&str> = candidate.split('/').filter(|s| !s.is_empty()).collect();
    p.len() == c.len() && p.iter().zip(&c).all(|(a, b)| *a == "*" || a == b)
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
    // `workspace_members` holds package IDs; `members` in the manifest is
    // a different (path-ish) shape. Membership is decided by matching
    // `packages[].id` against this set.
    let members = meta
        .get("workspace_members")
        .map(|m| m.str_set())
        .unwrap_or_default();
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

/// Parses one manifest and appends its target roots. Any failure to read
/// or interpret the manifest is a structured problem, never a silent skip.
fn add_targets(
    fs: &GitFs,
    manifest: &str,
    roots: &mut Vec<TargetRoot>,
    problems: &mut Vec<RootProblem>,
) {
    let mut problem = |message: String| {
        problems.push(RootProblem {
            manifest: manifest.to_owned(),
            message,
        });
    };
    let Some(text) = fs.read(manifest) else {
        problem("manifest is unreadable or missing".into());
        return;
    };
    let Ok(doc) = text.parse::<toml::Table>() else {
        problem("manifest is not parseable TOML".into());
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
    // Declared feature names, for `required-features` validation.
    let features: Vec<String> = doc
        .get("features")
        .and_then(|f| f.as_table())
        .map(|t| t.keys().map(|k| k.to_owned()).collect())
        .unwrap_or_default();

    // [lib] — explicit path (manifest-relative, joined to the package
    // directory) or the default location.
    let lib = doc.get("lib").and_then(|t| t.as_table());
    let lib_path = match lib.and_then(|t| t.get("path")).and_then(|p| p.as_str()) {
        Some(raw) => join_rel(&dir, raw),
        None => format!("{dir}/src/lib.rs"),
    };
    if exists(&lib_path) {
        roots.push(TargetRoot {
            path: lib_path,
            kind: RootKind::Lib,
            package: package.clone(),
        });
    }

    // build.rs — [package] build key (manifest-relative) or the
    // conventional file.
    let build_path = match pkg.get("build").and_then(|b| b.as_str()) {
        Some(raw) => join_rel(&dir, raw),
        None => format!("{dir}/build.rs"),
    };
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
        add_explicit(
            &PkgCtx {
                fs,
                dir: &dir,
                package: &package,
                manifest,
            },
            doc.get(key),
            &features,
            &spec,
            roots,
            problems,
        );
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

/// Context of the package currently being scanned, shared by explicit
/// target handling.
struct PkgCtx<'a> {
    fs: &'a GitFs,
    dir: &'a str,
    package: &'a str,
    manifest: &'a str,
}

/// Adds targets from one explicit table array, falling back to auto-
/// discovery of the conventional directory when the array is absent.
///
/// `required-features` policy (fail closed, documented in README): a
/// target with non-empty required-features is a root only when every
/// named feature is declared in `[features]` — otherwise cargo would
/// never build it and, more importantly, nothing guarantees it exists;
/// undeclared names are a `BAD_MANIFEST` problem. Auto-discovered
/// targets cannot carry required-features and are unaffected.
fn add_explicit(
    pkg: &PkgCtx,
    entries: Option<&toml::Value>,
    features: &[String],
    spec: &TargetSpec,
    roots: &mut Vec<TargetRoot>,
    problems: &mut Vec<RootProblem>,
) {
    let kind = spec.kind;
    let dir = pkg.dir;
    let exists = |p: &str| pkg.fs.exists(p);
    let mut seen: Vec<String> = Vec::new();
    if let Some(arr) = entries.and_then(|e| e.as_array()) {
        for t in arr {
            let Some(path) = t.get("path").and_then(|p| p.as_str()) else {
                continue;
            };
            let required: Vec<&str> = t
                .get("required-features")
                .and_then(|f| f.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let undeclared: Vec<&str> = required
                .iter()
                .filter(|f| !features.iter().any(|d| d == *f))
                .copied()
                .collect();
            if !undeclared.is_empty() {
                problems.push(RootProblem {
                    manifest: pkg.manifest.to_owned(),
                    message: format!(
                        "target kind `{kind:?}` requires undeclared features: {}",
                        undeclared.join(", ")
                    ),
                });
                continue;
            }
            let path = join_rel(dir, path);
            if exists(&path) && !seen.contains(&path) {
                seen.push(path.clone());
                roots.push(TargetRoot {
                    path,
                    kind,
                    package: pkg.package.to_owned(),
                });
            }
        }
    }
    if !spec.auto {
        return;
    }
    // Conventional auto-discovery: files directly in the dir, plus
    // `<dir>/<name>/main.rs` — the subdirectory target form exists ONLY
    // for immediate children (`src/bin/x/main.rs`), never any nested
    // path ending in `/main.rs` (e.g. `tests/case/crash_matrix.rs`
    // mounted by `tests/crash_recovery.rs` is helper data, not a root).
    let prefix = format!("{dir}/{}/", spec.conv_dir);
    for f in pkg.fs.tracked_files().unwrap_or_default() {
        if !f.starts_with(&prefix) || !f.ends_with(".rs") || seen.contains(&f) {
            continue;
        }
        let rest = &f[prefix.len()..];
        let is_root =
            !rest.contains('/') || (rest.ends_with("/main.rs") && rest.matches('/').count() == 1);
        if is_root && !seen.contains(&f.to_owned()) {
            seen.push(f.clone());
            roots.push(TargetRoot {
                path: f,
                kind,
                package: pkg.package.to_owned(),
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
