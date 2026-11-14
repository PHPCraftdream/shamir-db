//! Repo-level scan configuration (`.rust-guard/scan.toml`).
//!
//! ```toml
//! exclude_paths = ["tools/rust-guard/tests/fixtures"]
//! ```
//!
//! `exclude_paths` are EXACT repo-relative path prefixes (no wildcards):
//! roots, the module walk and the conservation inventory all skip files
//! under any excluded prefix. This is an explicit repo setting — the
//! modules gate must never hard-code repository-specific paths.
//!
//! Self-tests still scan fixture trees because they are materialised
//! into throwaway temp repos that carry no `.rust-guard/scan.toml`.

use serde::Deserialize;

/// Parsed scan configuration; a missing file is an empty configuration.
#[derive(Debug, Deserialize, Default)]
pub struct ScanCfg {
    #[serde(default)]
    pub exclude_paths: Vec<String>,
}

impl ScanCfg {
    /// Loads `.rust-guard/scan.toml` from the repo root (the `--repo`
    /// directory). Scan configuration is working-tree state, independent
    /// of any `--rev`.
    pub fn load(repo: &std::path::Path) -> Result<Self, String> {
        let path = repo.join(".rust-guard").join("scan.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        let parsed: Self =
            toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
        Ok(parsed)
    }

    /// True when `path` (repo-relative, `/` separators) sits under any
    /// excluded prefix.
    pub fn excluded(&self, path: &str) -> bool {
        self.exclude_paths.iter().any(|p| {
            let p = p.trim_end_matches('/');
            !p.is_empty() && (path == p || path.starts_with(&format!("{p}/")))
        })
    }
}
