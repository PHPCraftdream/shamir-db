//! `exceptions.toml` model: load, lookup, and stale detection.
//!
//! Default location: `.rust-guard/exceptions.toml` in the repo root.
//! Two record kinds exist:
//!
//! ```toml
//! [[exception]]
//! path = "crates/x/src/foo/tests/legacy.rs"  # exact path, no wildcards
//! package = "shamir-x"
//! reason = "generated-mount integration fixture"
//! owner = "@team"
//!
//! [[producer]]
//! name = "my_module_macro"   # macro (or OUT_DIR include) that emits modules
//! packages = ["shamir-x"]
//! owner = "@team"
//! reason = "codegen for wire types"
//! ```
//!
//! Stale exceptions (path no longer tracked, or the file is accounted for
//! without the exception) are themselves findings.

use serde::Deserialize;

/// One file exempted from the `INLINE_TEST_MOD` rule.
#[derive(Debug, Deserialize)]
pub struct InlineTests {
    pub path: String,
    pub reason: String,
    pub owner: String,
}

/// One exempted path: an exact file, never a wildcard.
#[derive(Debug, Deserialize)]
pub struct Exception {
    pub path: String,
    pub package: String,
    pub reason: String,
    pub owner: String,
}

/// A declared producer of unresolvable modules/includes (macro or OUT_DIR).
#[derive(Debug, Deserialize)]
pub struct Producer {
    pub name: String,
    #[serde(default)]
    pub packages: Vec<String>,
    pub owner: String,
    pub reason: String,
}
/// Parsed exceptions file.
#[derive(Debug, Deserialize, Default)]
pub struct Exceptions {
    #[serde(default, rename = "exception")]
    pub exceptions: Vec<Exception>,
    #[serde(default)]
    pub producers: Vec<Producer>,
    #[serde(default, rename = "inline_tests")]
    pub inline_tests: Vec<InlineTests>,
}

impl Exceptions {
    /// Loads the file at `path`; a missing file is an empty set (the tool
    /// still fails on unaccounted files, so an empty file is safe).
    pub fn load(path: &str) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("cannot read {path}: {e}")),
        };
        let parsed: Self =
            toml::from_str(&text).map_err(|e| format!("cannot parse {path}: {e}"))?;
        parsed.validate(path)?;
        Ok(parsed)
    }

    /// Structural sanity: every producer must name at least one package.
    fn validate(&self, path: &str) -> Result<(), String> {
        for p in &self.producers {
            if p.packages.is_empty() {
                return Err(format!("{path}: [[producer]] {} lists no packages", p.name));
            }
        }
        Ok(())
    }

    /// Returns the exception for an exact path, if declared.
    pub fn by_path(&self, path: &str) -> Option<&Exception> {
        self.exceptions.iter().find(|e| e.path == path)
    }

    /// The `[[inline_tests]]` entry for `path`, if declared.
    pub fn inline_tests_for(&self, path: &str) -> Option<&InlineTests> {
        self.inline_tests.iter().find(|e| e.path == path)
    }

    /// True when a macro/include producer with this name is declared.
    pub fn has_producer(&self, name: &str) -> bool {
        self.producers.iter().any(|p| p.name == name)
    }
}
