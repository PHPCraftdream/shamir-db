// Four `..` are needed to escape: from crates/fix/src, three `..`
// (crates/fix/src -> crates/fix -> crates -> repo root) land at the repo
// root — that spelling would resolve to a repo-root-level `outside.rs`
// and be reported as a plain Missing, not an escape. The four-dot chain
// pops PAST the top, so normalize_rel fails and PATH_ESCAPES_REPO must
// be reported; the escaped spelling must never become a visited key
// (real-repo finding 2).
#[path = "../../../../outside.rs"]
mod esc;
