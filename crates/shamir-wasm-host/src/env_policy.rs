//! Policy for seeding OS environment variables into [`GlobalVars`].
//!
//! Environment variables often hold secrets (API keys, tokens, passwords).
//! The default policy exposes **only** `SHAMIR_*` variables — a safe
//! least-privilege baseline. Broadening the policy is the operator's
//! explicit choice.

/// Composable filter deciding which OS environment variables are seeded
/// into the `env.*` namespace of [`GlobalVars`](super::GlobalVars).
///
/// All rules are **unioned** (OR semantics): a variable is included if it
/// matches *any* rule. The `SHAMIR_` prefix is always included and cannot
/// be turned off.
///
/// # Examples
///
/// ```
/// use shamir_wasm_host::EnvPolicy;
///
/// let p = EnvPolicy::default();
/// assert!(p.includes("SHAMIR_DB_PATH"));
/// assert!(!p.includes("PATH"));
/// ```
#[derive(Debug, Clone)]
pub struct EnvPolicy {
    /// Include every environment variable.
    pub all: bool,
    /// Extra name prefixes beyond the built-in `SHAMIR_`.
    pub prefixes: Vec<String>,
    /// Exact variable names to include.
    pub names: Vec<String>,
    /// Glob masks (`*` matches zero or more characters).
    pub masks: Vec<String>,
}

impl EnvPolicy {
    /// Whether `name` passes the policy (union of all rules).
    pub fn includes(&self, name: &str) -> bool {
        // Built-in: SHAMIR_ prefix is always included.
        if name.starts_with("SHAMIR_") {
            return true;
        }
        if self.all {
            return true;
        }
        if self.prefixes.iter().any(|p| name.starts_with(p.as_str())) {
            return true;
        }
        if self.names.iter().any(|n| n == name) {
            return true;
        }
        if self.masks.iter().any(|m| glob_matches(m, name)) {
            return true;
        }
        false
    }
}

impl Default for EnvPolicy {
    /// Safe default: `SHAMIR_*` only.
    fn default() -> Self {
        Self {
            all: false,
            prefixes: Vec::new(),
            names: Vec::new(),
            masks: Vec::new(),
        }
    }
}

/// Tiny `*`-only glob matcher — no external dependency.
///
/// A pattern is split at `*` into literal segments that must appear
/// consecutively in `text`. A single `*` matches zero or more characters.
pub(crate) fn glob_matches(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.is_empty() {
        return true;
    }
    // Leading empty string → pattern starts with *
    // Trailing empty string → pattern ends with *
    let mut cursor = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[cursor..].find(part) {
            Some(pos) => {
                // First segment must anchor to start if pattern doesn't start with *
                if i == 0 && !pattern.starts_with('*') && pos != 0 {
                    return false;
                }
                cursor += pos + part.len();
            }
            None => return false,
        }
    }
    // If pattern doesn't end with *, the last segment must anchor to end
    if !pattern.ends_with('*') && cursor != text.len() {
        return false;
    }
    true
}
