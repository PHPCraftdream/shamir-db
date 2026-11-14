//! `.rust-guard/layers.toml` + `.rust-guard/layers_baseline.toml`.
//!
//! `layers.toml` (human-edited): per crate an ordered list of layers,
//! lowest first; a module may refer to its own layer and lower ones
//! only. `layers_baseline.toml` (machine-managed by --update-baseline):
//! per crate a ratchet map `"A -> B" = files`, the number of production
//! files in layer A with at least one backward reference into layer B.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const DEFAULT_CONFIG: &str = ".rust-guard/layers.toml";
pub(crate) const DEFAULT_BASELINE: &str = ".rust-guard/layers_baseline.toml";

/// Human-edited layering configuration.
#[derive(Debug, Deserialize)]
pub(crate) struct LayersFile {
    #[serde(default)]
    pub(crate) crates: Vec<CrateLayers>,
}

/// One configured crate with its ordered layers (lowest first).
#[derive(Debug, Deserialize)]
pub(crate) struct CrateLayers {
    pub(crate) name: String,
    pub(crate) layers: Vec<String>,
    /// Layer name -> module path (`/`-separated). A layer absent from
    /// the map IS its own module path.
    #[serde(default)]
    pub(crate) modules: BTreeMap<String, String>,
}

/// Machine-managed ratchet baseline.
#[derive(Debug, Deserialize, serde::Serialize, Default)]
pub(crate) struct BaselineFile {
    #[serde(default)]
    pub(crate) crates: BTreeMap<String, BTreeMap<String, u64>>,
}

/// Loads the layering config (a working-tree file, cwd-relative like the
/// exceptions file). An empty `crates` list is fine (no-op gate).
pub(crate) fn load_config(path: &str) -> Result<Vec<CrateLayers>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let parsed: LayersFile = toml::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    Ok(parsed.crates)
}

/// Loads the ratchet baseline. A MISSING file is an error (fail closed:
/// without it every pair would look "new").
pub(crate) fn load_baseline(path: &str) -> Result<BaselineFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    toml::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

/// Splits a baseline key `"A -> B"` into its pair; `None` when malformed.
pub(crate) fn parse_pair(key: &str) -> Option<(String, String)> {
    let (a, b) = key.split_once(" -> ")?;
    if a.is_empty() || b.is_empty() {
        return None;
    }
    Some((a.to_owned(), b.to_owned()))
}

/// Canonical baseline key for a layer pair.
pub(crate) fn pair_key(a: &str, b: &str) -> String {
    format!("{a} -> {b}")
}

/// True when every `/`-segment of a layer name is a valid identifier.
pub(crate) fn layer_name_valid(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.split('/').all(|seg| {
        let mut chars = seg.chars();
        match chars.next() {
            Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
            _ => return false,
        }
        chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    })
}

/// Validates one crate's layering against its baseline section; returns
/// problem messages (caller turns them into `LAYER_BASELINE_BAD`
/// findings).
pub(crate) fn validate_crate(
    crate_cfg: &CrateLayers,
    baseline: Option<&BTreeMap<String, u64>>,
) -> Vec<String> {
    let mut problems = Vec::new();
    if crate_cfg.layers.is_empty() {
        problems.push(format!("crate `{}` declares no layers", crate_cfg.name));
    }
    let mut seen = BTreeSet::new();
    for layer in &crate_cfg.layers {
        if !layer_name_valid(layer) {
            problems.push(format!(
                "crate `{}`: invalid layer name `{layer}`",
                crate_cfg.name
            ));
        }
        if !seen.insert(layer.as_str()) {
            problems.push(format!(
                "crate `{}`: duplicate layer `{layer}`",
                crate_cfg.name
            ));
        }
    }
    let mut seen_mod_paths = BTreeSet::new();
    for (layer, path) in &crate_cfg.modules {
        if !crate_cfg.layers.contains(layer) {
            problems.push(format!(
                "crate `{}`: `modules` key `{layer}` is not a declared layer",
                crate_cfg.name
            ));
        }
        if !layer_name_valid(path) {
            problems.push(format!(
                "crate `{}`: invalid module path `{path}` for layer `{layer}`",
                crate_cfg.name
            ));
        }
        if !seen_mod_paths.insert(path.as_str()) {
            problems.push(format!(
                "crate `{}`: duplicate module path `{path}`",
                crate_cfg.name
            ));
        }
    }
    let Some(entries) = baseline else {
        return problems;
    };
    for key in entries.keys() {
        let Some((a, b)) = parse_pair(key) else {
            problems.push(format!(
                "crate `{}`: baseline key `{key}` is not a layer pair",
                crate_cfg.name
            ));
            continue;
        };
        for side in [a, b] {
            if !crate_cfg.layers.contains(&side) {
                problems.push(format!(
                    "crate `{}`: baseline pair `{key}` names undeclared layer `{side}`",
                    crate_cfg.name
                ));
            }
        }
    }
    problems
}

/// Maps a logical module path to its layer: the (name, index) of the
/// LONGEST layer whose MAPPED module path (see `CrateLayers::modules`)
/// equals the path or is a segment-wise prefix of it. Length is counted
/// in `/`-segments; duplicates were rejected at validation, so the
/// longest match is unique (a sub-layer like `repo/instance` wins over
/// `repo`). Pairs and baseline keys use LAYER NAMES, never module paths.
pub(crate) fn layer_of(
    path: &[String],
    layers: &[String],
    modules: &BTreeMap<String, String>,
) -> Option<(String, usize)> {
    let mut best: Option<(usize, String, usize)> = None;
    for (idx, layer) in layers.iter().enumerate() {
        let mapped = modules.get(layer).map(String::as_str).unwrap_or(layer);
        let prefix: Vec<&str> = mapped.split('/').collect();
        if prefix.iter().any(|s| s.is_empty()) {
            continue;
        }
        if path.len() < prefix.len()
            || !path[..prefix.len()]
                .iter()
                .zip(&prefix)
                .all(|(p, s)| p == s)
        {
            continue;
        }
        let longer = match &best {
            Some((_, _, n)) => prefix.len() > *n,
            None => true,
        };
        if longer {
            best = Some((idx, layer.clone(), prefix.len()));
        }
    }
    best.map(|(idx, name, _)| (name, idx))
}
