pub(in crate::boot::config::diagnostics::backup_online) mod destination;
pub(crate) mod retention;
pub(in crate::boot::config::diagnostics::backup_online) mod retention_pass;
pub(crate) mod runner;

#[cfg(test)]
mod tests;

// Test-only engine barrier-step hook plumbing: its OWN `#[cfg(test)]` module
// (a NEW path; the file stays in `tests/`, so the folder entry count is
// unchanged), so the `tests` module keeps its HEAD visibility and this one is
// a new module item. The sibling `registry` re-exports its helpers.
#[cfg(test)]
#[path = "tests/online_backup_step_hooks.rs"]
pub(crate) mod online_backup_step_hooks;
